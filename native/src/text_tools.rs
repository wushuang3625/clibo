use crate::model::TEXT_BUDGET;

pub fn transform(text: &str, mode: &str) -> Result<String, String> {
    let result = match mode {
        "original" => text.to_owned(),
        "trim" => text.trim().to_owned(),
        "blank-lines" => {
            let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let mut blank = false;
            text.split(newline)
                .filter(|line| {
                    let current = line.trim().is_empty();
                    let keep = !current || !blank;
                    blank = current;
                    keep
                })
                .collect::<Vec<_>>()
                .join(newline)
        }
        "upper" => text.to_uppercase(),
        "lower" => text.to_lowercase(),
        "json" => {
            let value: serde_json::Value = serde_json::from_str(text)
                .map_err(|_| "内容不是有效的 JSON，请选择其他处理方式")?;
            serde_json::to_string_pretty(&value).map_err(|_| "JSON 格式化失败")?
        }
        _ => return Err("不支持的文字处理方式".into()),
    };
    if result.len() > TEXT_BUDGET {
        return Err("处理结果超过 16 MiB，请减少内容后重试".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transformations_preserve_original_and_numeric_precision() {
        let source = "  SELECT 用户\r\n\r\n\r\n  FROM 表;  ";
        assert_eq!(transform(source, "original").unwrap(), source);
        assert_eq!(
            transform(source, "trim").unwrap(),
            "SELECT 用户\r\n\r\n\r\n  FROM 表;"
        );
        assert_eq!(
            transform(source, "blank-lines").unwrap(),
            "  SELECT 用户\r\n\r\n  FROM 表;  "
        );
        assert_eq!(transform("Straße 中文", "upper").unwrap(), "STRASSE 中文");
        assert!(transform("{invalid}", "json").is_err());
        let number = "123456789012345678901234567890";
        assert!(transform(&format!("{{\"id\":{number}}}"), "json")
            .unwrap()
            .contains(number));
        assert!(transform("hello", "unsupported").is_err());
    }
}
