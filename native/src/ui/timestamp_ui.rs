use super::*;

fn db() -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())
}

fn to_date(input: &str, millis: bool, offset: i64) -> Result<String, String> {
    let value = input
        .trim()
        .parse::<i64>()
        .map_err(|_| "请输入整数时间戳".to_string())?;
    let ms = if millis {
        value
    } else {
        value.checked_mul(1000).ok_or("时间戳超出范围")?
    };
    let seconds = ms
        .div_euclid(1000)
        .checked_add(offset * 3600)
        .ok_or("时间戳超出范围")?;
    let date: Option<String> = db()?
        .query_row(
            "SELECT strftime('%Y-%m-%d %H:%M:%S', ?1, 'unixepoch')",
            [seconds],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let date = date
        .filter(|v| v.len() == 19 && !v.starts_with("0000"))
        .ok_or("日期超出支持范围（0001–9999 年）")?;
    Ok(if millis {
        format!("{date}.{:03}", ms.rem_euclid(1000))
    } else {
        date
    })
}

fn to_stamp(input: &str, millis: bool, offset: i64) -> Result<String, String> {
    let input = input.trim();
    let (date, fraction) = input.split_once('.').unwrap_or((input, ""));
    if date.len() != 19 || !date.is_ascii() {
        return Err("格式应为 YYYY-MM-DD HH:mm:ss，可追加 .SSS 毫秒".into());
    }
    let b = date.as_bytes();
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b' ' | b'T')
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err("格式应为 YYYY-MM-DD HH:mm:ss".into());
    }
    let number = |a, z| {
        date[a..z]
            .parse::<u32>()
            .map_err(|_| "日期中包含无效数字".to_string())
    };
    let (y, m, d, h, min, s) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if y == 0 || d == 0 || d > days || h > 23 || min > 59 || s > 59 {
        return Err("日期或时间不存在，请检查年月日与时分秒".into());
    }
    if fraction.len() > 3
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (input.contains('.') && fraction.is_empty())
    {
        return Err("毫秒小数应为 1–3 位数字".into());
    }
    let sub = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<3}").parse::<i64>().unwrap()
    };
    let seconds: i64 = db()?
        .query_row("SELECT unixepoch(?1)", [date], |row| row.get(0))
        .map_err(|_| "日期无法解析".to_string())?;
    let seconds = seconds - offset * 3600;
    Ok(if millis {
        (seconds * 1000 + sub).to_string()
    } else {
        seconds.to_string()
    })
}

pub(super) struct TimestampPage {
    pub active: bool,
    live_ms: bool,
    paused: bool,
    current: i64,
    batch: bool,
    stamp: String,
    stamp_ms: bool,
    stamp_zone: i64,
    date: String,
    date_ms: bool,
    date_zone: i64,
    stamp_result: Result<String, String>,
    date_result: Result<String, String>,
    batch_input: String,
    batch_output: String,
    batch_reverse: bool,
    batch_ms: bool,
    batch_zone: i64,
    notice: String,
}
impl Default for TimestampPage {
    fn default() -> Self {
        let current = crate::model::now();
        Self {
            active: std::env::var_os("CLIBO_DATA_DIR").is_some()
                && std::env::args().any(|a| a == "--smoke")
                && std::env::args().any(|a| a == "--timestamp"),
            live_ms: false,
            paused: false,
            current,
            batch: false,
            stamp: current.to_string(),
            stamp_ms: true,
            stamp_zone: 8,
            date: to_date(&current.to_string(), true, 8).unwrap_or_default(),
            date_ms: true,
            date_zone: 8,
            stamp_result: Ok(String::new()),
            date_result: Ok(String::new()),
            batch_input: String::new(),
            batch_output: String::new(),
            batch_reverse: false,
            batch_ms: true,
            batch_zone: 8,
            notice: String::new(),
        }
    }
}
fn unit(ui: &mut egui::Ui, id: &str, ms: &mut bool) -> bool {
    let old = *ms;
    egui::ComboBox::from_id_salt(id)
        .selected_text(if *ms { "毫秒 (ms)" } else { "秒 (s)" })
        .show_ui(ui, |ui| {
            ui.selectable_value(ms, true, "毫秒 (ms)");
            ui.selectable_value(ms, false, "秒 (s)");
        });
    old != *ms
}
fn zone(ui: &mut egui::Ui, id: &str, offset: &mut i64) -> bool {
    let old = *offset;
    egui::ComboBox::from_id_salt(id)
        .width(130.)
        .selected_text(if *offset == 8 {
            "北京 · UTC+08:00"
        } else {
            "UTC · UTC+00:00"
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(offset, 8, "北京 · UTC+08:00");
            ui.selectable_value(offset, 0, "UTC · UTC+00:00");
        });
    old != *offset
}
fn output(ui: &mut egui::Ui, result: &Result<String, String>, p: &Palette) -> Option<String> {
    ui.label(RichText::new("转换结果").small().color(p.text_dim));
    let mut copy = None;
    ui.horizontal(|ui| {
        egui::Frame::NONE
            .fill(p.input)
            .stroke(egui::Stroke::new(1., p.hairline))
            .corner_radius(5)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 90.).max(150.));
                match result {
                    Ok(text) => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(if text.is_empty() {
                                    "等待转换"
                                } else {
                                    text
                                })
                                .monospace()
                                .color(if text.is_empty() { p.text_dim } else { p.text }),
                            )
                            .selectable(true),
                        );
                    }
                    Err(error) => {
                        ui.colored_label(p.error, error);
                    }
                }
            });
        if ui
            .add_enabled(
                result.as_ref().is_ok_and(|s| !s.is_empty()),
                egui::Button::new("复制"),
            )
            .clicked()
        {
            copy = result.as_ref().ok().cloned();
        }
    });
    copy
}
impl App {
    pub(super) fn timestamp_page(&mut self, ctx: &egui::Context) {
        let p = palette(self.dark);
        let mut copy = None;
        if !self.timestamp.paused {
            self.timestamp.current = crate::model::now();
            ctx.request_repaint_after(std::time::Duration::from_millis(
                if self.timestamp.live_ms { 50 } else { 250 },
            ));
        }
        egui::TopBottomPanel::top("timestamp-header")
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(10))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("←").clicked() {
                        self.timestamp.active = false;
                        self.focus_search = true;
                        self.dirty = true;
                    }
                    ui.separator();
                    ui.label(RichText::new("时间戳转换").size(26.).strong().color(p.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("－").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        let drag = ui.allocate_response(ui.available_size(), egui::Sense::drag());
                        if drag.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                    });
                });
                ui.label(RichText::new("秒与毫秒，日期与时间，一次转换清楚").color(p.text_dim));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(10))
            .show(ctx, |ui| {
                ui.spacing_mut().button_padding = Vec2::new(12., 8.);
                ui.spacing_mut().interact_size.y = 36.;
                ui.spacing_mut().item_spacing.y = 4.;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let card = || {
                        egui::Frame::NONE
                            .fill(p.card)
                            .stroke(egui::Stroke::new(1., p.border))
                            .corner_radius(8)
                            .inner_margin(10)
                    };
                    card().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new("当前时间戳").color(p.text_dim));
                        ui.horizontal_wrapped(|ui| {
                            let current = if self.timestamp.live_ms {
                                self.timestamp.current
                            } else {
                                self.timestamp.current.div_euclid(1000)
                            }
                            .to_string();
                            ui.label(
                                RichText::new(&current)
                                    .monospace()
                                    .size(28.)
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.label(if self.timestamp.live_ms {
                                "毫秒"
                            } else {
                                "秒"
                            });
                            if ui.button("切换单位").clicked() {
                                self.timestamp.live_ms = !self.timestamp.live_ms;
                            }
                            if ui.button("复制").clicked() {
                                copy = Some(current);
                            }
                            if ui
                                .button(if self.timestamp.paused {
                                    "继续"
                                } else {
                                    "暂停"
                                })
                                .clicked()
                            {
                                self.timestamp.paused = !self.timestamp.paused;
                            }
                        });
                    });
                    ui.add_space(6.);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.timestamp.batch, false, "单个转换");
                        ui.selectable_value(&mut self.timestamp.batch, true, "批量转换");
                    });
                    ui.add_space(10.);
                    if !self.timestamp.batch {
                        card().show(ui, |ui| {
                            ui.label(
                                RichText::new("01  时间戳转日期时间")
                                    .size(17.)
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.add_space(6.);
                            ui.label("时间戳 / 单位 / 时区");
                            ui.horizontal_wrapped(|ui| {
                                let mut changed = ui
                                    .add(
                                        egui::TextEdit::singleline(&mut self.timestamp.stamp)
                                            .margin(egui::Margin::symmetric(8, 8))
                                            .min_size(Vec2::new(0., 36.))
                                            .desired_width((ui.available_width() - 470.).max(120.)),
                                    )
                                    .changed();
                                changed |= unit(ui, "stamp-unit", &mut self.timestamp.stamp_ms);
                                changed |= zone(ui, "stamp-zone", &mut self.timestamp.stamp_zone);
                                if ui.button("填入当前").clicked() {
                                    let now = crate::model::now();
                                    self.timestamp.stamp = if self.timestamp.stamp_ms {
                                        now
                                    } else {
                                        now.div_euclid(1000)
                                    }
                                    .to_string();
                                    changed = true;
                                }
                                if changed {
                                    self.timestamp.stamp_result = Ok(String::new());
                                }
                                if primary(ui, &p, "转换").clicked() {
                                    self.timestamp.stamp_result = to_date(
                                        &self.timestamp.stamp,
                                        self.timestamp.stamp_ms,
                                        self.timestamp.stamp_zone,
                                    );
                                }
                            });
                            ui.add_space(6.);
                            if let Some(text) = output(ui, &self.timestamp.stamp_result, &p) {
                                copy = Some(text);
                            }
                        });
                        ui.add_space(10.);
                        card().show(ui, |ui| {
                            ui.label(
                                RichText::new("02  日期时间转时间戳")
                                    .size(17.)
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.add_space(6.);
                            ui.label("日期时间 · YYYY-MM-DD HH:mm:ss[.SSS]");
                            ui.horizontal_wrapped(|ui| {
                                let mut changed = ui
                                    .add(
                                        egui::TextEdit::singleline(&mut self.timestamp.date)
                                            .margin(egui::Margin::symmetric(8, 8))
                                            .min_size(Vec2::new(0., 36.))
                                            .desired_width((ui.available_width() - 470.).max(120.)),
                                    )
                                    .changed();
                                changed |= unit(ui, "date-unit", &mut self.timestamp.date_ms);
                                changed |= zone(ui, "date-zone", &mut self.timestamp.date_zone);
                                if ui.button("填入当前").clicked() {
                                    self.timestamp.date = to_date(
                                        &crate::model::now().to_string(),
                                        true,
                                        self.timestamp.date_zone,
                                    )
                                    .unwrap_or_default();
                                    changed = true;
                                }
                                if changed {
                                    self.timestamp.date_result = Ok(String::new());
                                }
                                if primary(ui, &p, "转换").clicked() {
                                    self.timestamp.date_result = to_stamp(
                                        &self.timestamp.date,
                                        self.timestamp.date_ms,
                                        self.timestamp.date_zone,
                                    );
                                }
                            });
                            ui.add_space(6.);
                            if let Some(text) = output(ui, &self.timestamp.date_result, &p) {
                                copy = Some(text);
                            }
                        });
                    } else {
                        card().show(ui, |ui| {
                            ui.label(
                                RichText::new("批量转换 · 每行一条")
                                    .size(17.)
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.horizontal_wrapped(|ui| {
                                let mut changed = ui
                                    .selectable_value(
                                        &mut self.timestamp.batch_reverse,
                                        false,
                                        "时间戳 → 日期",
                                    )
                                    .changed();
                                changed |= ui
                                    .selectable_value(
                                        &mut self.timestamp.batch_reverse,
                                        true,
                                        "日期 → 时间戳",
                                    )
                                    .changed();
                                changed |= unit(ui, "batch-unit", &mut self.timestamp.batch_ms);
                                changed |= zone(ui, "batch-zone", &mut self.timestamp.batch_zone);
                                if changed {
                                    self.timestamp.batch_output.clear();
                                }
                            });
                            egui::ScrollArea::vertical()
                                .id_salt("timestamp-batch-input")
                                .max_height(180.)
                                .show(ui, |ui| {
                                    if ui
                                        .add(
                                            egui::TextEdit::multiline(
                                                &mut self.timestamp.batch_input,
                                            )
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(6)
                                            .hint_text("粘贴内容，每行一个时间戳或日期时间"),
                                        )
                                        .changed()
                                    {
                                        self.timestamp.batch_output.clear();
                                    }
                                });
                            ui.horizontal(|ui| {
                                if primary(ui, &p, "转换全部").clicked() {
                                    self.timestamp.batch_output = self
                                        .timestamp
                                        .batch_input
                                        .lines()
                                        .enumerate()
                                        .map(|(i, line)| {
                                            let result = if self.timestamp.batch_reverse {
                                                to_stamp(
                                                    line,
                                                    self.timestamp.batch_ms,
                                                    self.timestamp.batch_zone,
                                                )
                                            } else {
                                                to_date(
                                                    line,
                                                    self.timestamp.batch_ms,
                                                    self.timestamp.batch_zone,
                                                )
                                            };
                                            result.unwrap_or_else(|error| {
                                                format!("第 {} 行：{error}", i + 1)
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                }
                                if ui
                                    .add_enabled(
                                        !self.timestamp.batch_output.is_empty(),
                                        egui::Button::new("复制全部结果"),
                                    )
                                    .clicked()
                                {
                                    copy = Some(self.timestamp.batch_output.clone());
                                }
                            });
                            egui::ScrollArea::both()
                                .id_salt("timestamp-batch-output")
                                .max_height(250.)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&self.timestamp.batch_output).monospace(),
                                        )
                                        .selectable(true),
                                    );
                                });
                        });
                    }
                    ui.add_space(6.);
                    ui.label(
                        RichText::new("时区采用固定 UTC 偏移；秒输出舍去毫秒部分。")
                            .small()
                            .color(p.text_dim),
                    );
                    if !self.timestamp.notice.is_empty() {
                        ui.label(RichText::new(&self.timestamp.notice).color(p.accent));
                    }
                });
            });
        if let Some(text) = copy {
            self.timestamp.notice = match platform::write(Some(&text), None, self.owner) {
                Ok(()) => "已复制".into(),
                Err(error) => error,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epoch_milliseconds_and_offsets() {
        assert_eq!(to_date("0", false, 0).unwrap(), "1970-01-01 00:00:00");
        assert_eq!(to_date("-1", true, 0).unwrap(), "1969-12-31 23:59:59.999");
        assert_eq!(
            to_stamp("2026-09-10 00:00:00", true, 8).unwrap(),
            "1788969600000"
        );
        assert_eq!(
            to_date("1788969600000", true, 8).unwrap(),
            "2026-09-10 00:00:00.000"
        );
        assert_eq!(to_stamp("1970-01-01T00:00:00.1", true, 0).unwrap(), "100");
    }
    #[test]
    fn rejects_invalid_dates_and_overflow() {
        for value in [
            "2025-02-29 00:00:00",
            "2024-04-31 00:00:00",
            "2024-01-01 24:00:00",
            "2024-01-01 00:00:00.",
            "2024-01-01 00:00:00.1234",
            "garbage",
        ] {
            assert!(to_stamp(value, true, 0).is_err(), "{value}");
        }
        assert!(to_stamp("2024-02-29 00:00:00", true, 0).is_ok());
        assert!(to_date(&i64::MAX.to_string(), false, 0).is_err());
        for value in ["-2208988800123", "0", "1789020468711"] {
            assert_eq!(
                to_stamp(&to_date(value, true, 8).unwrap(), true, 8).unwrap(),
                value
            );
        }
    }
}
