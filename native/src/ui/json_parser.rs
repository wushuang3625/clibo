use serde_json::Value;

/// Accept data literals only. The normalized result still passes through serde's
/// JSON grammar and depth limit; no expression or Python code is evaluated.
pub(super) fn parse(source: &str) -> Result<Value, String> {
    if let Ok(value) = serde_json::from_str(source) {
        return Ok(value);
    }
    let normalized = normalize(source)?;
    serde_json::from_str(&normalized).map_err(|error| {
        format!("无法解析：{error}（兼容转换后的行列）；请检查原文引号、逗号和括号")
    })
}

fn normalize(source: &str) -> Result<String, String> {
    let mut chars = source.chars().peekable();
    let mut output = String::with_capacity(source.len());
    while let Some(c) = chars.next() {
        if c == '\'' || c == '"' {
            let quote = c;
            let mut encoded = String::from("\"");
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    c if c == quote => {
                        closed = true;
                        break;
                    }
                    '\\' => {
                        let next = chars.next().ok_or("字符串末尾缺少转义字符")?;
                        match next {
                            // Python quote escapes and Markdown escaped underscores.
                            '\'' => encoded.push('\''),
                            '_' => encoded.push('_'),
                            '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u' => {
                                encoded.push('\\');
                                encoded.push(next);
                            }
                            // Python preserves unknown escapes, e.g. Windows paths.
                            _ if quote == '\'' => {
                                encoded.push_str("\\\\");
                                encoded.push(next);
                            }
                            _ => return Err(format!("不支持的字符串转义：\\{next}")),
                        }
                    }
                    '"' => encoded.push_str("\\\""),
                    c if c.is_control() => return Err("字符串中存在未转义的换行或控制字符".into()),
                    c => encoded.push(c),
                }
            }
            if !closed {
                return Err("字符串引号未闭合".into());
            }
            encoded.push('"');
            // Validate escapes before handing the full document to serde.
            serde_json::from_str::<String>(&encoded).map_err(|e| format!("字符串转义错误：{e}"))?;
            output.push_str(&encoded);
        } else if c.is_ascii_alphabetic() || c == '_' {
            let mut word = String::from(c);
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                word.push(chars.next().unwrap());
            }
            output.push_str(match word.as_str() {
                "True" => "true",
                "False" => "false",
                "None" => "null",
                _ => &word,
            });
        } else {
            output.push(c);
        }
    }
    Ok(output)
}

/// Expand objects/arrays encoded as strings, leaving scalar strings (IDs,
/// timestamps, booleans) unchanged. Bound total depth across decoded layers.
pub(super) fn expand(value: &mut Value, depth: usize) -> usize {
    if depth >= 64 {
        return 0;
    }
    let mut count = 0;
    if let Value::String(text) = value {
        let trimmed = text.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(decoded @ (Value::Object(_) | Value::Array(_))) = parse(trimmed) {
                *value = decoded;
                count += 1;
            }
        }
    }
    match value {
        Value::Object(object) => {
            for child in object.values_mut() {
                count += expand(child, depth + 1);
            }
        }
        Value::Array(array) => {
            for child in array {
                count += expand(child, depth + 1);
            }
        }
        _ => {}
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dictionary_with_multiple_json_layers() {
        let source = r#"{'rsp\_data': '{"code":0,"timestamp":1789002331499,"data":[{"g":"25","a":6.0}]}', 'rqs\_param': '{"rqs\_data": {"data":"{\\"osh\\":\\"720\\",\\"pi\\":\\"{}\\"}","pcode":"10031"}}'}"#;
        let mut value = parse(source).unwrap();
        assert!(value["rsp_data"].is_string());
        assert_eq!(expand(&mut value, 0), 4);
        assert_eq!(value["rsp_data"]["data"][0]["g"], "25");
        assert_eq!(value["rqs_param"]["rqs_data"]["data"]["osh"], "720");
        assert!(value["rqs_param"]["rqs_data"]["data"]["pi"].is_object());
    }

    #[test]
    fn quotes_literals_and_precision_are_preserved() {
        let value = parse(r#"{'text': 'it\'s "fine"', 'ok': True, 'no': False, 'nil': None, 'n': 123456789012345678901234567890}"#).unwrap();
        assert_eq!(value["text"], "it's \"fine\"");
        assert_eq!(value["ok"], true);
        assert_eq!(value["no"], false);
        assert!(value["nil"].is_null());
        assert_eq!(value["n"].to_string(), "123456789012345678901234567890");
        let mut value = parse(r#"{"path":"C:\\temp\\a", "text":"don't", "id":"001", "literal":"a\\_b", "bad":"{broken}"}"#).unwrap();
        assert_eq!(expand(&mut value, 0), 0);
        assert_eq!(value["literal"], r"a\_b");
        assert_eq!(value["id"], "001");
        assert_eq!(value["path"], r"C:\temp\a");
    }

    #[test]
    fn rejects_expressions_and_incomplete_input() {
        for text in [
            "{'a': __import__('os')}",
            "{'a': 'unfinished}",
            "{'a': 1} trailing",
            "{'a': (1,2)}",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
