use super::*;
use serde_json::Value;

#[derive(Default)]
pub(super) struct JsonPage {
    pub active: bool,
    pub source: String,
    formatted: String,
    value: Option<Value>,
    query: String,
    error: String,
    notice: String,
    preserve_strings: bool,
    expanded: usize,
    compatible: bool,
    collapsed: HashSet<String>,
    all_closed: bool,
    elapsed_ms: f64,
    pub find_open: bool,
    find_text: String,
    find_index: usize,
    find_scroll: bool,
    pub history_open: bool,
    history_tab: usize,
    history_items: Vec<JsonHistoryView>,
    history_error: String,
    history_search: String,
}

impl JsonPage {
    pub(super) fn initial() -> Self {
        let mut page = Self::default();
        if std::env::var_os("CLIBO_DATA_DIR").is_some()
            && std::env::args().any(|a| a == "--smoke")
            && std::env::args().any(|a| a == "--json")
        {
            page.active = true;
            page.history_open = std::env::args().any(|a| a == "--history");
            page.source = serde_json::to_string_pretty(&serde_json::json!({
                "event": "clipboard.exported",
                "request_id": "demo-20260910-001",
                "user": {"name": "Example User", "locale": "zh-CN", "notifications": true},
                "items": [
                    {"type": "text", "label": "SQL 查询", "size": 128},
                    {"type": "link", "label": "Documentation", "url": "https://example.com/docs"}
                ],
                "settings": {"theme": "light", "capture_images": true, "retention_days": 30}
            }))
            .unwrap();
            page.parse();
        }
        page
    }
    pub(super) fn parse(&mut self) {
        let started = std::time::Instant::now();
        self.collapsed.clear();
        self.value = None;
        self.formatted.clear();
        self.error.clear();
        self.notice.clear();
        self.expanded = 0;
        self.compatible = false;
        if self.source.trim().is_empty() {
            return;
        }
        match super::json_parser::parse(&self.source) {
            Ok(mut value) => {
                self.compatible = serde_json::from_str::<Value>(&self.source).is_err();
                if !self.preserve_strings {
                    self.expanded = super::json_parser::expand(&mut value, 0);
                }
                self.formatted = serde_json::to_string_pretty(&value).unwrap();
                self.value = Some(value);
            }
            Err(error) => {
                self.error = error;
            }
        }
        self.elapsed_ms = started.elapsed().as_secs_f64() * 1000.;
    }
}

#[derive(Default)]
pub(super) struct WindowTransition {
    active: bool,
    original: Option<egui::Rect>,
    animation: Option<(std::time::Instant, egui::Rect, egui::Rect)>,
}

impl WindowTransition {
    pub(super) fn take_original(&mut self) -> Option<egui::Rect> {
        self.active = false;
        let original = self
            .original
            .take()
            .or_else(|| self.animation.map(|(_, _, target)| target));
        self.animation = None;
        original
    }
    pub(super) fn update(&mut self, ctx: &egui::Context, active: bool, owner: usize) {
        if self.active != active {
            self.active = active;
            let current = ctx.input(|i| i.viewport().inner_rect);
            if let Some(current) = current {
                let target = if active {
                    self.original = Some(current);
                    let mut work = ctx.input(|i| {
                        egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            i.viewport().monitor_size.unwrap_or(Vec2::new(1920., 1080.)),
                        )
                    });
                    #[cfg(windows)]
                    unsafe {
                        use windows_sys::Win32::Graphics::Gdi::{
                            GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
                            MONITOR_DEFAULTTONEAREST,
                        };
                        let monitor = MonitorFromWindow(owner as _, MONITOR_DEFAULTTONEAREST);
                        let mut info: MONITORINFO = std::mem::zeroed();
                        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                        if GetMonitorInfoW(monitor, &mut info) != 0 {
                            let scale = ctx.pixels_per_point();
                            work = egui::Rect::from_min_max(
                                egui::pos2(
                                    info.rcWork.left as f32 / scale,
                                    info.rcWork.top as f32 / scale,
                                ),
                                egui::pos2(
                                    info.rcWork.right as f32 / scale,
                                    info.rcWork.bottom as f32 / scale,
                                ),
                            );
                        }
                    }
                    let size = (current.size() * 2.).min(work.size());
                    let min = (current.center() - size / 2.).clamp(work.min, work.max - size);
                    egui::Rect::from_min_size(min, size)
                } else {
                    self.original.take().unwrap_or(current)
                };
                self.animation = Some((std::time::Instant::now(), current, target));
            }
        }
        if let Some((started, from, to)) = self.animation {
            let t = (started.elapsed().as_secs_f32() / 0.30).min(1.);
            let eased = t * t * (3. - 2. * t);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                from.size() + (to.size() - from.size()) * eased,
            ));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                from.min + (to.min - from.min) * eased,
            ));
            if t >= 1. {
                self.animation = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
    }
}

// Bracket notation keeps keys containing dots, quotes and Unicode unambiguous.
fn fields(value: &Value, path: &str, query: &str, found: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let path = format!("{path}[{}]", serde_json::to_string(key).unwrap());
                if key.to_lowercase().contains(query) {
                    let summary = match child {
                        Value::Object(v) => format!("对象 · {} 个字段", v.len()),
                        Value::Array(v) => format!("数组 · {} 项", v.len()),
                        _ => child.to_string(),
                    };
                    found.push((path.clone(), summary));
                }
                fields(child, &path, query, found);
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter().enumerate() {
                fields(child, &format!("{path}[{index}]"), query, found);
            }
        }
        _ => {}
    }
}

impl App {
    fn copy_json_text(&mut self, text: &str) {
        self.json.notice = match platform::write(Some(text), None, self.owner) {
            Ok(()) => "已复制".into(),
            Err(error) => error,
        };
    }

    fn refresh_json_history(&mut self) {
        let result = self.backend.store.lock().unwrap().json_history();
        match result {
            Ok(items) => {
                self.json.history_items = items;
                self.json.history_error.clear();
            }
            Err(error) => self.json.history_error = error,
        }
    }

    pub(super) fn remember_json_history(&mut self) {
        if self.json.value.is_none() || self.json.source.trim().is_empty() {
            return;
        }
        let source = self.json.source.clone();
        let result = self.backend.store.lock().unwrap().add_json_history(&source);
        match result {
            Ok(()) => {
                if self.json.history_open {
                    self.refresh_json_history();
                }
            }
            Err(error) => self.json.notice = error,
        }
    }

    fn open_json_history(&mut self) {
        self.remember_json_history();
        self.refresh_json_history();
        self.json.history_open = true;
    }

    fn load_json_source(&mut self, source: String) {
        self.json.source = source;
        self.json.parse();
        self.json.history_open = false;
    }

    fn history_tab_button(
        ui: &mut egui::Ui,
        p: &Palette,
        label: &str,
        active: bool,
    ) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(label).size(12.5).strong().color(if active {
                p.accent
            } else {
                p.text_dim
            }))
            .min_size(Vec2::new(118., 32.))
            .fill(if active { p.accent_soft } else { p.input })
            .stroke(egui::Stroke::new(
                1.,
                if active { p.accent } else { p.border },
            ))
            .corner_radius(CornerRadius::same(8)),
        )
    }

    fn history_row(
        ui: &mut egui::Ui,
        p: &Palette,
        row_id: egui::Id,
        time: &str,
        tag: &str,
        source: &str,
        preview: &str,
    ) -> egui::Response {
        const ROW_HEIGHT: f32 = 72.;
        const TIME_WIDTH: f32 = 110.;
        const SOURCE_WIDTH: f32 = 190.;
        const COLUMN_GAP: f32 = 18.;
        const ARROW_WIDTH: f32 = 28.;

        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), egui::Sense::click());
        let hover = ui
            .ctx()
            .animate_bool_with_time(row_id.with("hover"), response.hovered(), 0.08);
        let fill = mix(p.card, p.card_hover, hover);
        let border = mix(p.border, p.accent, hover * 0.35);
        let corner = CornerRadius::same(8);
        ui.painter()
            .add(egui::Shape::rect_filled(rect, corner, fill));
        ui.painter().add(egui::Shape::rect_stroke(
            rect,
            corner,
            egui::Stroke::new(1., border),
            egui::StrokeKind::Inside,
        ));

        let inner = rect.shrink2(Vec2::new(18., 10.));
        let time_end = inner.left() + TIME_WIDTH;
        let source_start = time_end + COLUMN_GAP;
        let source_end = source_start + SOURCE_WIDTH;
        let content_start = source_end + COLUMN_GAP;
        let content_end = (inner.right() - ARROW_WIDTH).max(content_start);

        let divider_y = egui::Rangef::new(inner.top() + 5., inner.bottom() - 5.);
        for x in [time_end + COLUMN_GAP / 2., source_end + COLUMN_GAP / 2.] {
            ui.painter().line_segment(
                [egui::pos2(x, divider_y.min), egui::pos2(x, divider_y.max)],
                egui::Stroke::new(1., p.hairline),
            );
        }

        let time_rect = egui::Rect::from_min_max(inner.min, egui::pos2(time_end, inner.bottom()));
        let mut time_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(row_id.with("time"))
                .max_rect(time_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        time_ui.add(egui::Label::new(mono(time).size(11.5).color(p.text_dim)).truncate());

        let source_rect = egui::Rect::from_min_max(
            egui::pos2(source_start, inner.top()),
            egui::pos2(source_end, inner.bottom()),
        );
        let mut source_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(row_id.with("source"))
                .max_rect(source_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        kind_tag(&mut source_ui, p, tag);
        source_ui.add_space(7.);
        source_ui.add(
            egui::Label::new(RichText::new(source).size(12.).strong().color(p.text)).truncate(),
        );

        let content_rect = egui::Rect::from_min_max(
            egui::pos2(content_start, inner.top()),
            egui::pos2(content_end, inner.bottom()),
        );
        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(row_id.with("content"))
                .max_rect(content_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let lines = preview
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect::<Vec<_>>();
        let mut display = if lines.is_empty() {
            "（空内容）".to_string()
        } else {
            lines.join("\n")
        };
        if display.chars().count() > 280 {
            display = display.chars().take(279).collect();
            display.push('…');
        }
        content_ui.add_sized(
            content_rect.size(),
            egui::Label::new(RichText::new(display).size(12.3).color(p.text)).wrap(),
        );

        ui.painter().text(
            egui::pos2(rect.right() - 18., rect.center().y),
            egui::Align2::CENTER_CENTER,
            "›",
            egui::FontId::proportional(21.),
            if response.hovered() {
                p.accent
            } else {
                p.text_dim
            },
        );

        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn json_history_page(&mut self, ctx: &egui::Context) {
        let p = palette(self.dark, self.theme);
        egui::TopBottomPanel::top("json-history-header")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("←").size(18.).color(p.text))
                                .min_size(Vec2::new(38., 38.))
                                .fill(p.input)
                                .stroke(egui::Stroke::new(1., p.border))
                                .corner_radius(CornerRadius::same(8)),
                        )
                        .on_hover_text("返回 JSON 工作页")
                        .clicked()
                    {
                        self.json.history_open = false;
                    }
                    ui.add_space(8.);
                    ui.label(mono("{ }").size(28.).strong().color(p.accent));
                    ui.add_space(8.);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("历史记录").size(25.).strong().color(p.text));
                        ui.add_space(1.);
                        ui.label(
                            RichText::new("点击一条记录，直接回到 JSON 工作页查看和编辑")
                                .size(12.)
                                .color(p.text_dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new("－")
                                    .min_size(Vec2::new(30., 30.))
                                    .fill(p.input)
                                    .stroke(egui::Stroke::new(1., p.border))
                                    .corner_radius(CornerRadius::same(7)),
                            )
                            .on_hover_text("最小化")
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
                ui.add_space(14.);
                ui.horizontal(|ui| {
                    if Self::history_tab_button(ui, &p, "剪贴板历史", self.json.history_tab == 0)
                        .clicked()
                    {
                        self.json.history_tab = 0;
                    }
                    ui.add_space(6.);
                    if Self::history_tab_button(ui, &p, "JSON 历史", self.json.history_tab == 1)
                        .clicked()
                    {
                        self.json.history_tab = 1;
                        self.refresh_json_history();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::NONE
                            .fill(p.input)
                            .stroke(egui::Stroke::new(1., p.border))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(10, 3))
                            .show(ui, |ui| {
                                ui.add_sized(
                                    Vec2::new(240., 26.),
                                    egui::TextEdit::singleline(&mut self.json.history_search)
                                        .frame(false)
                                        .hint_text("搜索历史记录…"),
                                );
                            });
                    });
                });
            });

        let needle = self.json.history_search.trim().to_lowercase();
        let clipboard_items = if self.json.history_tab == 0 {
            self.backend
                .store
                .lock()
                .unwrap()
                .entries
                .iter()
                .filter(|entry| entry.view.kind != "image")
                .filter(|entry| {
                    needle.is_empty()
                        || entry.view.preview.to_lowercase().contains(&needle)
                        || entry.view.source.to_lowercase().contains(&needle)
                })
                .map(|entry| entry.view.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let json_items = if self.json.history_tab == 1 {
            self.json
                .history_items
                .iter()
                .filter(|item| needle.is_empty() || item.preview.to_lowercase().contains(&needle))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut selected_clipboard = None;
        let mut selected_json = None;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ctx, |ui| {
                if !self.json.history_error.is_empty() {
                    egui::Frame::NONE
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1., p.error))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.colored_label(p.error, &self.json.history_error);
                        });
                    ui.add_space(10.);
                }

                egui::ScrollArea::vertical()
                    .id_salt("json-history-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let empty = if self.json.history_tab == 0 {
                            clipboard_items.is_empty()
                        } else {
                            json_items.is_empty()
                        };
                        if empty && self.json.history_error.is_empty() {
                            ui.add_space(72.);
                            ui.vertical_centered(|ui| {
                                ui.label(mono("{ }").size(30.).strong().color(p.accent));
                                ui.add_space(8.);
                                ui.label(
                                    RichText::new(if needle.is_empty() {
                                        if self.json.history_tab == 0 {
                                            "还没有可用的文本剪贴板历史"
                                        } else {
                                            "还没有 JSON 历史"
                                        }
                                    } else {
                                        "没有找到匹配的历史记录"
                                    })
                                    .size(14.)
                                    .strong()
                                    .color(p.text),
                                );
                                ui.add_space(4.);
                                ui.label(
                                    RichText::new(if self.json.history_tab == 0 {
                                        "复制文本后会自动出现在这里"
                                    } else {
                                        "格式化或打开有效 JSON 后会自动记录"
                                    })
                                    .size(11.5)
                                    .color(p.text_dim),
                                );
                            });
                            return;
                        }

                        if self.json.history_tab == 0 {
                            for item in &clipboard_items {
                                let source = if item.source.trim().is_empty() {
                                    "未知来源"
                                } else {
                                    item.source.as_str()
                                };
                                let tag = if item.kind == "link" { "URL" } else { "TXT" };
                                let response = Self::history_row(
                                    ui,
                                    &p,
                                    egui::Id::new(("clipboard-history-row", &item.id)),
                                    &time_label(item.copied_at),
                                    tag,
                                    source,
                                    &item.preview,
                                );
                                if response.clicked() {
                                    selected_clipboard = Some(item.id.clone());
                                }
                                ui.add_space(9.);
                            }
                        } else {
                            for item in &json_items {
                                let source =
                                    format!("JSON 工作页 · {:.1} KB", item.bytes as f64 / 1024.);
                                let response = Self::history_row(
                                    ui,
                                    &p,
                                    egui::Id::new(("json-history-row", &item.id)),
                                    &time_label(item.created_at),
                                    "JSON",
                                    &source,
                                    &item.preview,
                                );
                                if response.clicked() {
                                    selected_json = Some(item.id.clone());
                                }
                                ui.add_space(9.);
                            }
                        }
                    });
            });

        if let Some(id) = selected_clipboard {
            let source = self
                .backend
                .store
                .lock()
                .unwrap()
                .secret(&id)
                .and_then(|secret| {
                    secret
                        .text
                        .ok_or_else(|| "这条剪贴板记录没有文本内容".into())
                });
            match source {
                Ok(source) => {
                    self.load_json_source(source);
                    self.remember_json_history();
                }
                Err(error) => self.json.history_error = error,
            }
        } else if let Some(id) = selected_json {
            let source = self.backend.store.lock().unwrap().json_history_source(&id);
            match source {
                Ok(source) => self.load_json_source(source),
                Err(error) => self.json.history_error = error,
            }
        }
    }

    pub(super) fn json_page(&mut self, ctx: &egui::Context) {
        if self.json.history_open {
            self.json_history_page(ctx);
            return;
        }
        let p = palette(self.dark, self.theme);
        let focus_find = self.consume_find_shortcut(ctx);
        if focus_find {
            self.json.find_open = true;
        }
        if self.json.find_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.json.find_open = false;
        }
        egui::TopBottomPanel::top("json-header")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(20, 14)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().button_padding = Vec2::new(12., 7.);
                ui.horizontal(|ui| {
                    if ui.button("←").clicked() {
                        self.remember_json_history();
                        self.json.active = false;
                        self.focus_search = true;
                        self.dirty = true;
                    }
                    ui.separator();
                    ui.label(mono("{ }").size(30.).color(p.accent));
                    ui.label(RichText::new("JSON 工作页").size(26.).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("－").on_hover_text("最小化").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        let response =
                            ui.allocate_response(ui.available_size(), egui::Sense::drag());
                        if response.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                    });
                });
                ui.add_space(4.);
                ui.label(
                    RichText::new("整理复制内容，快速找到嵌套字段")
                        .size(15.)
                        .color(p.text_dim),
                );
                ui.add_space(10.);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("格式化").clicked() {
                        self.json.parse();
                        if self.json.value.is_some() {
                            // Format the outer literal while retaining string-valued JSON.
                            if let Ok(value) = super::json_parser::parse(&self.json.source) {
                                self.json.source = serde_json::to_string_pretty(&value).unwrap();
                            }
                            self.remember_json_history();
                        }
                    }
                    if ui.button("历史记录").clicked() {
                        self.open_json_history();
                    }
                    if ui
                        .add_enabled(
                            self.json.value.is_some(),
                            egui::Button::new("▣  复制格式化结果"),
                        )
                        .clicked()
                    {
                        self.copy_json_text(&self.json.formatted.clone());
                    }
                    if ui
                        .add_enabled(
                            self.json.value.is_some(),
                            egui::Button::new("▣  复制压缩结果"),
                        )
                        .clicked()
                    {
                        self.copy_json_text(&self.json.value.as_ref().unwrap().to_string());
                    }
                    if ui.button("清空").clicked() {
                        self.remember_json_history();
                        self.json = JsonPage {
                            active: true,
                            ..Default::default()
                        };
                    }
                });
                ui.add_space(5.);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .checkbox(&mut self.json.preserve_strings, "保留内层 JSON 字符串")
                        .changed()
                    {
                        self.json.parse();
                    }
                    if self.json.expanded > 0 {
                        ui.label(
                            RichText::new(format!(
                                "已展开 {} 处 · 复制结果使用展开结构",
                                self.json.expanded
                            ))
                            .small()
                            .color(p.text_dim),
                        );
                    }
                    if !self.json.notice.is_empty() {
                        ui.label(RichText::new(&self.json.notice).color(p.accent));
                    }
                });
                if self.json.find_open {
                    ui.horizontal(|ui| {
                        ui.label("全文查找");
                        let input = ui.add(
                            egui::TextEdit::singleline(&mut self.json.find_text)
                                .id_salt("json-find")
                                .hint_text("查找字段或值…")
                                .desired_width(240.),
                        );
                        if focus_find {
                            input.request_focus();
                        }
                        if input.changed() {
                            self.json.find_index = 0;
                            self.json.find_scroll = true;
                        }
                        let count = super::json_view::find_matches(
                            &self.json.formatted,
                            &self.json.find_text,
                        )
                        .len();
                        self.json.find_index = self.json.find_index.min(count.saturating_sub(1));
                        ui.label(format!(
                            "{} / {}",
                            if count == 0 {
                                0
                            } else {
                                self.json.find_index + 1
                            },
                            count
                        ));
                        let previous = ui
                            .add_enabled(count > 0, egui::Button::new("↑ 上一个"))
                            .clicked()
                            || ctx.input_mut(|i| {
                                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)
                            });
                        let next = ui
                            .add_enabled(count > 0, egui::Button::new("↓ 下一个"))
                            .clicked()
                            || ctx.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            });
                        if count > 0 && (previous || next) {
                            self.json.find_index = (self.json.find_index
                                + if previous { count - 1 } else { 1 })
                                % count;
                            self.json.find_scroll = true;
                        }
                        if ui.button("关闭 · Esc").clicked() {
                            self.json.find_open = false;
                        }
                    });
                }
            });
        egui::TopBottomPanel::bottom("json-status")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let valid = self.json.value.is_some();
                    ui.colored_label(
                        if valid {
                            Color32::from_rgb(65, 145, 70)
                        } else {
                            p.text_dim
                        },
                        if valid {
                            "●  JSON 有效"
                        } else {
                            "○  等待有效内容"
                        },
                    );
                    ui.separator();
                    ui.label(format!(
                        "字段：{}",
                        self.json
                            .value
                            .as_ref()
                            .map(super::json_view::field_count)
                            .unwrap_or(0)
                    ));
                    ui.separator();
                    ui.label(format!(
                        "大小：{:.1} KB",
                        self.json.source.len() as f64 / 1024.
                    ));
                    ui.separator();
                    ui.label(format!("耗时：{:.1} ms", self.json.elapsed_ms));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label("UTF-8");
                    });
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::TRANSPARENT).inner_margin(12)).show(ctx, |ui| {
            let height = ui.available_height();
            ui.spacing_mut().item_spacing.x = 12.;
            ui.columns(2, |columns| {
                let card = || egui::Frame::NONE.fill(p.card).stroke(egui::Stroke::new(1., p.border)).corner_radius(8).inner_margin(12);
                card().show(&mut columns[0], |ui| {
                    ui.set_min_height((height - 26.).max(100.));
                    ui.set_max_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("01  原文").size(19.).strong().color(p.accent));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(if self.json.compatible { "兼容字典" } else { "JSON" }).color(p.text_dim));
                        });
                    });
                    ui.label(RichText::new("粘贴 JSON，编辑后自动校验").color(p.text_dim));
                    ui.add_space(8.);
                    egui::Frame::NONE.fill(p.input).stroke(egui::Stroke::new(1., p.hairline)).inner_margin(8).show(ui, |ui| {
                        egui::ScrollArea::both().id_salt("json-source-scroll").auto_shrink([false, false]).max_height((height - 100.).max(60.)).show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                let count = self.json.source.split('\n').count();
                                let numbers = (1..=count).map(|n| format!("{n:>3}")).collect::<Vec<_>>().join("\n");
                                let mut job = super::json_view::highlight(&numbers, self.dark, self.theme);
                                for section in &mut job.sections { section.format.color = p.text_dim; }
                                ui.add(egui::Label::new(job).selectable(false).extend());
                                ui.separator();
                                let dark = self.dark;
                                let theme = self.theme;
                                let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _width: f32| {
                                    ui.fonts_mut(|fonts| fonts.layout_job(super::json_view::highlight(text.as_str(), dark, theme)))
                                };
                                let response = ui.add(egui::TextEdit::multiline(&mut self.json.source).code_editor().frame(false).margin(egui::Margin::ZERO).hint_text("在这里粘贴 JSON 或单引号字典…").desired_width(ui.available_width().max(200.)).desired_rows(25).layouter(&mut layouter));
                                if response.changed() { self.json.parse(); }
                            });
                        });
                    });
                });
                card().show(&mut columns[1], |ui| {
                    ui.set_min_height((height - 26.).max(100.));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("02  格式化 / 字段检索").size(19.).strong().color(p.accent));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("收起全部").clicked() { self.json.all_closed = true; self.json.collapsed.clear(); }
                            if ui.small_button("展开全部").clicked() { self.json.all_closed = false; self.json.collapsed.clear(); }
                        });
                    });
                    ui.add_space(5.);
                    ui.add(egui::TextEdit::singleline(&mut self.json.query).hint_text("搜索字段名，忽略大小写").desired_width(f32::INFINITY));
                    ui.add_space(4.);
                    if !self.json.error.is_empty() { ui.colored_label(p.error, &self.json.error); }
                    else if let Some(value) = &self.json.value {
                        let searching = !self.json.query.trim().is_empty() && !self.json.find_open;
                        let mut matches = Vec::new();
                        if searching { fields(value, "$", &self.json.query.trim().to_lowercase(), &mut matches); }
                        ui.label(RichText::new(if searching { format!("找到 {} 个字段", matches.len()) } else { format!("●  校验通过 · {} 行", self.json.formatted.lines().count()) }).color(p.text_dim));
                        egui::Frame::NONE.fill(p.input).stroke(egui::Stroke::new(1., p.hairline)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                            egui::ScrollArea::both().id_salt("json-tree-scroll").auto_shrink([false, false]).max_height((height - 142.).max(60.)).show(ui, |ui| {
                                if self.json.find_open {
                                    let matches = super::json_view::find_matches(&self.json.formatted, &self.json.find_text);
                                    let current = matches.get(self.json.find_index);
                                    let mut start = 0;
                                    for line in self.json.formatted.split('\n') {
                                        let end = start + line.len();
                                        let ranges: Vec<_> = matches.iter().filter(|r| r.start >= start && r.end <= end).map(|r| (r.start - start)..(r.end - start)).collect();
                                        let selected = current.is_some_and(|r| r.start >= start && r.start <= end);
                                        let local_current = current.filter(|_| selected).map(|r| (r.start - start)..(r.end - start)); let job = super::json_view::highlight_matches(line, self.dark, self.theme, &ranges, local_current.as_ref());
                                        let response = ui.add(egui::Label::new(job).selectable(true).extend());
                                        if selected && self.json.find_scroll { response.scroll_to_me(Some(egui::Align::Center)); }
                                        start = end + 1;
                                    }
                                    self.json.find_scroll = false;
                                } else if searching {
                                    if matches.is_empty() { ui.label("没有匹配字段，试试更短的关键词。"); }
                                    for (path, summary) in matches {
                                        ui.push_id(&path, |ui| {
                                            ui.horizontal(|ui| {
                                                if ui.small_button("复制路径").clicked() {
                                                    self.json.notice = match platform::write(Some(&path), None, self.owner) { Ok(()) => "已复制路径".into(), Err(e) => e };
                                                }
                                                ui.label(RichText::new(&path).monospace().color(p.accent));
                                            });
                                            ui.add(egui::Label::new(super::json_view::highlight(&summary, self.dark, self.theme)).selectable(true));
                                            ui.separator();
                                        });
                                    }
                                } else {
                                    super::json_view::tree(ui, value, None, "$", self.dark, self.theme, &mut self.json.collapsed, self.json.all_closed, false);
                                }
                            });
                        });
                    } else {
                        ui.add_space(40.);
                        ui.label(RichText::new("让杂乱的数据变得清晰").size(18.).color(p.text_dim));
                        ui.label("粘贴内容后，在这里展开结构、搜索字段。");
                    }
                });
            });
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_transition_restores_original_rect_after_enlarging() {
        let ctx = egui::Context::default();
        let original = egui::Rect::from_min_size(egui::pos2(20., 20.), Vec2::new(780., 580.));
        let mut transition = WindowTransition::default();
        let input = |rect| {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.inner_rect = Some(rect);
            viewport.monitor_size = Some(Vec2::new(1920., 1080.));
            input
        };
        let _ = ctx.run(input(original), |ctx| transition.update(ctx, true, 0));
        let (_, from, enlarged) = transition.animation.unwrap();
        assert_eq!(from, original);
        assert!(enlarged.width() <= original.width() * 2.);
        assert!(enlarged.height() <= original.height() * 2.);
        let _ = ctx.run(input(enlarged), |ctx| transition.update(ctx, false, 0));
        assert_eq!(transition.animation.unwrap().2, original);
        transition.animation.as_mut().unwrap().0 =
            std::time::Instant::now() - std::time::Duration::from_secs(1);
        let output = ctx.run(input(enlarged), |ctx| transition.update(ctx, false, 0));
        assert!(transition.animation.is_none());
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.iter().any(|command| matches!(command, egui::ViewportCommand::InnerSize(size) if *size == original.size())));
        assert!(commands.iter().any(|command| matches!(command, egui::ViewportCommand::OuterPosition(pos) if *pos == original.min)));
    }
    #[test]
    fn expanded_fields_are_searchable_and_can_be_restored_to_strings() {
        let mut page = JsonPage {
            source: r#"{'rsp_data': '{"data":[{"g":"25","a":6.0}]}', 'rqs_param': '{"data":"{\\"osh\\":\\"720\\"}"}'}"#.into(),
            ..Default::default()
        };
        page.parse();
        assert!(page.error.is_empty(), "{}", page.error);
        let mut found = Vec::new();
        fields(page.value.as_ref().unwrap(), "$", "osh", &mut found);
        assert_eq!(
            found,
            vec![(
                "$[\"rqs_param\"][\"data\"][\"osh\"]".into(),
                "\"720\"".into()
            )]
        );
        page.preserve_strings = true;
        page.parse();
        assert!(page.value.as_ref().unwrap()["rsp_data"].is_string());
        assert_eq!(page.expanded, 0);
    }
    #[test]
    fn nested_search_keeps_distinct_paths() {
        let value = serde_json::json!({"a.b": [{"Name": 1}, {"name": 2}], "名字": true});
        let mut found = Vec::new();
        fields(&value, "$", "name", &mut found);
        assert_eq!(
            found,
            vec![
                ("$[\"a.b\"][0][\"Name\"]".into(), "1".into()),
                ("$[\"a.b\"][1][\"name\"]".into(), "2".into())
            ]
        );
    }
    #[test]
    fn invalid_edit_clears_old_result_and_preserves_input() {
        let mut page = JsonPage {
            source: "{\"n\":123456789012345678901234567890}".into(),
            ..Default::default()
        };
        page.parse();
        assert!(page.formatted.contains("123456789012345678901234567890"));
        page.source = "{bad".into();
        page.parse();
        assert!(page.value.is_none());
        assert!(page.formatted.is_empty());
        assert!(!page.error.is_empty());
        assert_eq!(page.source, "{bad");
    }
}
