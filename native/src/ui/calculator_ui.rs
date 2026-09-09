use super::*;
use crate::calculator::Answer;
use rust_decimal::Decimal;
use std::time::{Duration, Instant};

fn is_numeric_clipboard_item(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && (Decimal::from_str_exact(text).is_ok() || Decimal::from_scientific(text).is_ok())
}

#[cfg(test)]
mod clipboard_filter_tests {
    use super::is_numeric_clipboard_item;

    #[test]
    fn keeps_only_numeric_clipboard_text() {
        for text in ["42", " -3.14 ", "1e6"] {
            assert!(is_numeric_clipboard_item(text), "{text}");
        }
        for text in ["", "1+2", "10%", "abc", "12 34"] {
            assert!(!is_numeric_clipboard_item(text), "{text}");
        }
    }
}

impl App {
    fn leave_calculator(&mut self) {
        self.calculator.leave();
        self.query.clear();
        self.focus_search = true;
        self.dirty = true;
    }

    fn copy_calculation(&mut self, text: String) {
        let message = match platform::write(Some(&text), None, self.owner) {
            Ok(()) => "已复制".into(),
            Err(_) => "复制失败，请重试".into(),
        };
        self.calculator.notice = Some((message, Instant::now()));
        self.calculator.focus = true;
    }

    fn calculator_clipboard_items(&self) -> Vec<ClipView> {
        self.backend
            .store
            .lock()
            .unwrap()
            .entries
            .iter()
            .take(20)
            .filter(|entry| {
                entry.view.kind == "text" && is_numeric_clipboard_item(&entry.view.preview)
            })
            .map(|entry| entry.view.clone())
            .collect()
    }

    fn use_clipboard_in_calculator(&mut self, id: &str) {
        let text = self
            .backend
            .store
            .lock()
            .unwrap()
            .secret(id)
            .and_then(|secret| secret.text.ok_or_else(|| "这条记录没有文本内容".into()));
        match text {
            Ok(text) => {
                self.calculator.expression.push_str(&text);
                self.calculator.confirmed = None;
                self.calculator.continuation_approximate = false;
                self.calculator.show_full = false;
                self.calculator.focus = true;
                self.calculator.notice = Some(("已追加复制内容".into(), Instant::now()));
            }
            Err(error) => self.backend.report(Err(error)),
        }
    }

    pub(super) fn calculator_page(&mut self, ctx: &egui::Context) {
        let p = palette(self.dark);
        let input_id = egui::Id::new("calculator-expression");
        let mut composition_event = false;
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Ime(event) = event {
                    composition_event = true;
                    match event {
                        egui::ImeEvent::Preedit(text) => self.ime_composing = !text.is_empty(),
                        egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => {
                            self.ime_composing = false
                        }
                        _ => {}
                    }
                }
            }
        });
        let keyboard = !self.settings_open && !self.ime_composing && !composition_event;
        if !self.ime_composing
            && !composition_event
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            if self.settings_open {
                if self.hotkey_capture.is_some() {
                    self.hotkey_capture = None;
                } else {
                    self.settings_open = false;
                    self.calculator.focus = true;
                }
            } else {
                self.leave_calculator();
                return;
            }
        }
        let input_focused = ctx.memory(|m| m.has_focus(input_id));
        let mut confirm = false;
        let mut copy = false;
        if keyboard && input_focused {
            copy = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter));
            confirm =
                !copy && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            if self.calculator.expression.is_empty()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
            {
                self.leave_calculator();
                return;
            }
            if self.calculator.confirmed.is_some() {
                let events = ctx.input(|i| i.events.clone());
                // Navigation or selection always wins over calculator-style first-key handling.
                let editing = events.iter().any(|e| matches!(e,
                    egui::Event::Key { key: egui::Key::ArrowLeft | egui::Key::ArrowRight |
                        egui::Key::Home | egui::Key::End | egui::Key::Backspace | egui::Key::Delete,
                        pressed: true, .. }) || matches!(e,
                    egui::Event::Key { key: egui::Key::A, modifiers, pressed: true, .. } if modifiers.command || modifiers.ctrl));
                if editing {
                    self.calculator.confirmed = None;
                } else if let Some(text) = events.iter().find_map(|e| match e {
                    egui::Event::Text(t) | egui::Event::Paste(t) => Some(t),
                    _ => None,
                }) {
                    let paste = events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
                    self.calculator.first_input(text, paste);
                    if let Some(mut state) = egui::TextEdit::load_state(ctx, input_id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(
                                    self.calculator.expression.chars().count(),
                                ),
                            )));
                        state.store(ctx, input_id);
                    }
                }
            }
        }
        egui::TopBottomPanel::top("calculator-brand")
            .exact_height(2.)
            .frame(egui::Frame::NONE.fill(p.accent))
            .show(ctx, |_| {});
        egui::TopBottomPanel::top("calculator-header")
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(mono("CLIBO").size(15.).strong().color(p.text));
                    ui.label(RichText::new("计算器").size(12.).color(p.text_dim));
                    let drag = ui.allocate_response(
                        Vec2::new(ui.available_width() - 260., 24.),
                        egui::Sense::drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    ui.label(RichText::new("计算中 · 已固定").size(11.).color(p.accent))
                        .on_hover_text("返回剪贴板后恢复原来的固定设置");
                    if icon_button(
                        ui,
                        &p,
                        if self.dark { "sun" } else { "moon" },
                        false,
                        "切换外观",
                    )
                    .clicked()
                    {
                        self.dark = !self.dark;
                        apply_theme(ctx, self.dark);
                        if let Ok(dir) = backend::data_dir() {
                            UiState { dark: self.dark }.save(&dir);
                        }
                    }
                    if icon_button(ui, &p, "settings", self.settings_open, "偏好设置").clicked()
                    {
                        self.settings = self.backend.store.lock().unwrap().settings.clone();
                        self.excluded = self.settings.excluded_apps.join("\n");
                        self.settings_open = true;
                        self.hotkey_capture = None;
                        self.capture_error.clear();
                    }
                    if icon_button(ui, &p, "hide", false, "收起窗口，保留当前计算").clicked()
                    {
                        self.hide(ctx);
                    }
                });
                ui.add_space(10.);
                ui.add_enabled_ui(!self.settings_open, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(mono("=").size(22.).strong().color(p.accent));
                        let output = egui::TextEdit::singleline(&mut self.calculator.expression)
                            .id(input_id)
                            .font(egui::TextStyle::Monospace)
                            .desired_width((ui.available_width() - 158.).max(120.))
                            .hint_text("输入算式，例如 128 * 3 + 56")
                            .show(ui);
                        if self.calculator.focus {
                            output.response.request_focus();
                            self.calculator.focus = false;
                            let mut state = output.state;
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(
                                        self.calculator.expression.chars().count(),
                                    ),
                                )));
                            state.store(ctx, input_id);
                        }
                        if output.response.changed() || output.response.clicked() {
                            self.calculator.confirmed = None;
                            self.calculator.show_full = false;
                        }
                        if ui.button("清空").clicked() {
                            self.calculator.expression.clear();
                            self.calculator.confirmed = None;
                            self.calculator.continuation_approximate = false;
                            self.calculator.focus = true;
                        }
                        if ui.button("返回剪贴板").clicked() {
                            self.leave_calculator();
                        }
                    });
                });
            });

        // egui's single-line editor strips line breaks from pasted text. Keep invalid
        // multi-line input intact for validation instead of calculating a changed value.
        if input_focused && !self.settings_open {
            if let Some(paste) = ctx.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Paste(text) if text.contains(['\n', '\r']) => Some(text.clone()),
                    _ => None,
                })
            }) {
                self.calculator.expression = paste;
                self.calculator.confirmed = None;
            }
        }
        let result = self.calculator.result();
        if keyboard {
            if let Ok(answer) = &result {
                if confirm {
                    self.calculator.confirm(answer.clone());
                }
                if copy {
                    self.copy_calculation(answer.full());
                }
            }
        }
        egui::TopBottomPanel::bottom("calculator-footer")
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let hint = if self.calculator.confirmed.is_some() {
                        "输入运算符继续 · 输入数字开始新计算"
                    } else {
                        "Enter 确认计算 · Shift+Enter 复制结果 · Esc 返回"
                    };
                    ui.label(RichText::new(hint).size(11.).color(p.text_dim));
                    if let Some((message, at)) = &self.calculator.notice {
                        if at.elapsed() < Duration::from_secs(3) {
                            ui.label(RichText::new(message).size(11.).color(p.accent));
                            ctx.request_repaint_after(Duration::from_millis(100));
                        }
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.add_enabled_ui(!self.settings_open, |ui| {
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1., p.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if self.calculator.expression.trim().is_empty() {
                                ui.label(RichText::new("开始计算").size(12.).color(p.text_dim));
                                ui.add_space(12.);
                                ui.label(
                                    RichText::new("输入算式，结果会实时显示")
                                        .size(23.)
                                        .color(p.text),
                                );
                                ui.add_space(16.);
                                ui.horizontal_wrapped(|ui| {
                                    for example in ["128*3+56", "(1200-200)/8"] {
                                        if ui.button(example).clicked() {
                                            self.calculator.expression = example.into();
                                            self.calculator.focus = true;
                                        }
                                    }
                                    if !self.calculator.draft.is_empty()
                                        && ui.button("恢复上次算式").clicked()
                                    {
                                        self.calculator.expression = self.calculator.draft.clone();
                                        self.calculator.focus = true;
                                    }
                                    if ui.small_button("按文本搜索 =").clicked() {
                                        self.leave_calculator();
                                        self.query = "\\=".into();
                                    }
                                });
                            } else {
                                match &result {
                                    Ok(answer) => self.calculator_result(ui, &p, answer),
                                    Err(error) => {
                                        ui.label(
                                            RichText::new(if error.incomplete {
                                                "需要补全"
                                            } else {
                                                "无法计算"
                                            })
                                            .size(12.)
                                            .color(p.text_dim),
                                        );
                                        ui.add_space(12.);
                                        ui.label(RichText::new(&error.message).size(22.).color(
                                            if error.incomplete {
                                                p.text_dim
                                            } else {
                                                p.error
                                            },
                                        ));
                                        ui.label(
                                            RichText::new(format!(
                                                "检查第 {} 个字符附近",
                                                error.position + 1
                                            ))
                                            .size(11.)
                                            .color(p.text_dim),
                                        );
                                        ui.add_space(12.);
                                        ui.horizontal(|ui| {
                                            ui.add_enabled(false, egui::Button::new("复制算式"));
                                            ui.add_enabled(false, egui::Button::new("复制结果"));
                                        });
                                    }
                                }
                            }
                        });
                    if self.calculator.expression.contains(['%', '％']) {
                        ui.label(
                            RichText::new("10% = 0.1；增加 10% 请写作 × (1 + 10%)")
                                .size(11.)
                                .color(p.text_dim),
                        );
                    }
                    ui.add_space(12.);
                    let clipboard_items = self.calculator_clipboard_items();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("最近复制").strong().color(p.text));
                        ui.label(
                            RichText::new("点击追加到算式 · 滚轮浏览")
                                .size(11.)
                                .color(p.text_dim),
                        );
                    });
                    ui.add_space(6.);
                    if clipboard_items.is_empty() {
                        egui::Frame::new()
                            .fill(p.card)
                            .stroke(egui::Stroke::new(1., p.border))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new("复制数字或算式后，会显示在这里")
                                        .size(12.)
                                        .color(p.text_dim),
                                );
                            });
                    } else {
                        ui.scope(|ui| {
                            // The list only scrolls horizontally. egui defaults to requiring
                            // Shift+wheel for horizontal scrolling, so map a normal mouse wheel
                            // to the only enabled direction inside this list.
                            ui.style_mut().always_scroll_the_only_direction = true;
                            egui::ScrollArea::horizontal()
                                .id_salt("calculator-clipboard")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for item in &clipboard_items {
                                            let preview = item.preview.replace(['\n', '\r'], " ");
                                            let label = if preview.trim().is_empty() {
                                                "空文本".to_string()
                                            } else {
                                                preview
                                            };
                                            let button = egui::Button::new(
                                                RichText::new(&label).size(12.).color(p.text),
                                            )
                                            .fill(p.card)
                                            .stroke(egui::Stroke::new(1., p.border))
                                            .corner_radius(CornerRadius::same(8));
                                            if ui
                                                .add_sized([180., 42.], button)
                                                .on_hover_text(format!(
                                                    "{} · {}\n点击追加到计算输入框",
                                                    item.source,
                                                    time_label(item.copied_at)
                                                ))
                                                .clicked()
                                            {
                                                self.use_clipboard_in_calculator(&item.id);
                                            }
                                        }
                                    });
                                });
                        });
                    }
                    ui.add_space(14.);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("最近计算").strong().color(p.text));
                        ui.label(RichText::new("仅本次运行").size(11.).color(p.text_dim));
                        if ui
                            .add_enabled(
                                !self.calculator.history.is_empty(),
                                egui::Button::new("清空记录"),
                            )
                            .clicked()
                        {
                            self.calculator.removed = Some((
                                std::mem::take(&mut self.calculator.history),
                                Instant::now(),
                            ));
                        }
                        if self
                            .calculator
                            .removed
                            .as_ref()
                            .is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(5))
                        {
                            ctx.request_repaint_after(Duration::from_millis(100));
                            if ui.button("撤销清空").clicked() {
                                if let Some((old, _)) = self.calculator.removed.take() {
                                    self.calculator.history.extend(old);
                                    self.calculator.history.truncate(50);
                                }
                            }
                        } else {
                            self.calculator.removed = None;
                        }
                    });
                    ui.add_space(6.);
                    egui::ScrollArea::vertical()
                        .id_salt("calculator-history")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.calculator.history.is_empty() {
                                ui.add_space(18.);
                                ui.label(
                                    RichText::new("确认后的计算会出现在这里")
                                        .size(13.)
                                        .color(p.text_dim),
                                );
                            }
                            let mut remove = None;
                            for (index, record) in
                                self.calculator.history.clone().iter().enumerate()
                            {
                                ui.push_id(index, |ui| {
                                    ui.horizontal(|ui| {
                                        let width = (ui.available_width() * 0.45).max(120.);
                                        if ui
                                            .add_sized(
                                                [width, 30.],
                                                egui::Button::new(mono(&record.expression))
                                                    .frame(false)
                                                    .truncate(),
                                            )
                                            .on_hover_text(&record.expression)
                                            .clicked()
                                        {
                                            self.calculator.expression = record.expression.clone();
                                            self.calculator.continuation_approximate =
                                                record.answer.approximate;
                                            self.calculator.confirmed = None;
                                            self.calculator.focus = true;
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.small_button("移除").clicked() {
                                                    remove = Some(index);
                                                }
                                                if ui.small_button("复制").clicked() {
                                                    self.copy_calculation(record.answer.full());
                                                }
                                                ui.add(
                                                    egui::Label::new(
                                                        mono(record.answer.display()).color(p.text),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(record.answer.full());
                                            },
                                        );
                                    });
                                    ui.separator();
                                });
                            }
                            if let Some(index) = remove {
                                self.calculator.history.remove(index);
                            }
                        });
                });
            });
        if self.calculator.focus || !self.calculator.active {
            ctx.request_repaint();
        }
    }

    fn calculator_result(&mut self, ui: &mut egui::Ui, p: &Palette, answer: &Answer) {
        ui.label(
            RichText::new(if self.calculator.confirmed.is_some() {
                "已确认"
            } else {
                "实时结果"
            })
            .size(12.)
            .color(p.text_dim),
        );
        ui.add_space(6.);
        let text = if self.calculator.show_full {
            answer.full()
        } else {
            answer.display()
        };
        egui::ScrollArea::horizontal()
            .id_salt("calculator-result")
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        mono(&text)
                            .size(if text.len() > 22 { 26. } else { 42. })
                            .color(p.text),
                    )
                    .selectable(true),
                );
            });
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(if self.calculator.show_full {
                    "收起位数"
                } else {
                    "更多位数"
                })
                .clicked()
            {
                self.calculator.show_full = !self.calculator.show_full;
            }
            if answer.approximate {
                ui.label(RichText::new("近似结果").size(11.).color(p.text_dim));
            }
            if ui.button("复制算式").clicked() {
                self.copy_calculation(format!(
                    "{} {} {}",
                    self.calculator.expression,
                    if answer.approximate { "≈" } else { "=" },
                    answer.full()
                ));
            }
            if primary(ui, p, "复制结果")
                .on_hover_text("复制完整结果 · Shift+Enter")
                .clicked()
            {
                self.copy_calculation(answer.full());
            }
        });
    }
}
