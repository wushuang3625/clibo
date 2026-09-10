use super::*;
use serde_json::Value;

pub(super) fn find_matches(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    text.to_ascii_lowercase()
        .match_indices(&query.to_ascii_lowercase())
        .map(|(start, matched)| start..start + matched.len())
        .collect()
}

pub(super) fn highlight_matches(
    text: &str,
    dark: bool,
    ranges: &[std::ops::Range<usize>],
    current: Option<&std::ops::Range<usize>>,
) -> egui::text::LayoutJob {
    let mut job = highlight(text, dark);
    let sections = std::mem::take(&mut job.sections);
    for section in sections {
        let mut boundaries = vec![section.byte_range.start, section.byte_range.end];
        for range in ranges {
            if range.start > section.byte_range.start && range.start < section.byte_range.end {
                boundaries.push(range.start);
            }
            if range.end > section.byte_range.start && range.end < section.byte_range.end {
                boundaries.push(range.end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        for pair in boundaries.windows(2) {
            let mut part = section.clone();
            part.byte_range = pair[0]..pair[1];
            if ranges.iter().any(|r| r.contains(&pair[0])) {
                part.format.background = if current.is_some_and(|r| r.contains(&pair[0])) {
                    Color32::from_rgb(242, 179, 68)
                } else {
                    Color32::from_rgb(245, 222, 142)
                };
                part.format.color = Color32::from_rgb(45, 35, 15);
            }
            job.sections.push(part);
        }
    }
    job
}

#[cfg(test)]
mod search_tests {
    use super::*;
    #[test]
    fn search_preserves_unicode_offsets_and_matches_values() {
        let text = "{\"名字\":\"成功\",\"Name\":\"name\"}";
        let found = find_matches(text, "NAME");
        assert_eq!(found.len(), 2);
        for range in &found {
            assert!(text.is_char_boundary(range.start));
        }
        assert_eq!(find_matches(text, "成功").len(), 1);
        assert!(find_matches(text, "").is_empty());
        let job = highlight_matches(text, false, &found, found.first());
        assert_eq!(job.text, text);
        assert_eq!(
            job.sections
                .iter()
                .filter(|s| s.format.background != Color32::TRANSPARENT)
                .count(),
            2
        );
    }
}

pub(super) fn highlight(text: &str, dark: bool) -> egui::text::LayoutJob {
    let p = palette(dark);
    let key = if dark {
        Color32::from_rgb(112, 172, 255)
    } else {
        Color32::from_rgb(0, 94, 220)
    };
    let string = if dark {
        Color32::from_rgb(244, 146, 137)
    } else {
        Color32::from_rgb(193, 49, 61)
    };
    let number = if dark {
        Color32::from_rgb(99, 205, 188)
    } else {
        Color32::from_rgb(0, 128, 124)
    };
    let mut job = egui::text::LayoutJob::default();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        let start = i;
        let mut color = p.text;
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            color = if text[i..].trim_start().starts_with(':') {
                key
            } else {
                string
            };
        } else if bytes[i].is_ascii_digit() || bytes[i] == b'-' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i])) {
                i += 1;
            }
            color = number;
        } else {
            i += text[i..].chars().next().unwrap().len_utf8();
        }
        job.append(
            &text[start..i],
            0.,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.),
                color,
                line_height: Some(21.),
                ..Default::default()
            },
        );
    }
    job
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tree(
    ui: &mut egui::Ui,
    value: &Value,
    key: Option<&str>,
    path: &str,
    dark: bool,
    collapsed: &mut HashSet<String>,
    all_closed: bool,
    comma: bool,
) {
    let prefix = key
        .map(|k| format!("{}: ", serde_json::to_string(k).unwrap()))
        .unwrap_or_default();
    let suffix = if comma { "," } else { "" };
    let (open, close, len) = match value {
        Value::Object(v) => ("{", "}", v.len()),
        Value::Array(v) => ("[", "]", v.len()),
        _ => {
            ui.horizontal(|ui| {
                ui.add_space(22.);
                ui.add(
                    egui::Label::new(highlight(&format!("{prefix}{value}{suffix}"), dark))
                        .selectable(true)
                        .extend(),
                );
            });
            return;
        }
    };
    let closed = all_closed ^ collapsed.contains(path);
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new(if closed { "▶" } else { "▼" }).color(palette(dark).accent),
                )
                .frame(false),
            )
            .clicked()
            && !collapsed.remove(path)
        {
            collapsed.insert(path.to_owned());
        }
        let line = if closed {
            format!("{prefix}{open} … {close}{suffix}")
        } else {
            format!("{prefix}{open}")
        };
        ui.add(
            egui::Label::new(highlight(&line, dark))
                .selectable(true)
                .extend(),
        );
        if closed {
            ui.label(
                RichText::new(format!("{len} 项"))
                    .small()
                    .color(palette(dark).text_dim),
            );
        }
    });
    if !closed {
        ui.indent(path, |ui| match value {
            Value::Object(object) => {
                for (index, (key, child)) in object.iter().enumerate() {
                    tree(
                        ui,
                        child,
                        Some(key),
                        &format!("{path}[{}]", serde_json::to_string(key).unwrap()),
                        dark,
                        collapsed,
                        all_closed,
                        index + 1 < len,
                    );
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    tree(
                        ui,
                        child,
                        None,
                        &format!("{path}[{index}]"),
                        dark,
                        collapsed,
                        all_closed,
                        index + 1 < len,
                    );
                }
            }
            _ => {}
        });
        ui.horizontal(|ui| {
            ui.add_space(22.);
            ui.add(egui::Label::new(highlight(&format!("{close}{suffix}"), dark)).selectable(true));
        });
    }
}

pub(super) fn field_count(value: &Value) -> usize {
    match value {
        Value::Object(v) => v.len() + v.values().map(field_count).sum::<usize>(),
        Value::Array(v) => v.iter().map(field_count).sum(),
        _ => 0,
    }
}
