use super::*;
use crate::launcher::{
    self,
    catalog::Catalog,
    search::{Action, Builtin, Results, Row},
    Entry,
};

struct SearchJob {
    id: u64,
    catalog: Arc<Catalog>,
    custom: Vec<Entry>,
    recent: Vec<String>,
    prefixes: crate::model::LauncherSearchPrefixes,
    query: String,
}
enum SearchDone {
    Results(u64, Results),
    Icon(String, Vec<u8>),
}
struct Worker {
    tx: mpsc::Sender<SearchJob>,
    rx: mpsc::Receiver<SearchDone>,
}
impl Worker {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, jobs) = mpsc::channel::<SearchJob>();
        let (done, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            let _shell = launcher::windows::ShellSession::new();
            let mut pending = None;
            let mut icons: HashMap<String, Option<Vec<u8>>> = HashMap::new();
            while let Some(mut job) = pending.take().or_else(|| jobs.recv().ok()) {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let results = launcher::search::search_with_prefixes(
                    &job.catalog,
                    &job.custom,
                    &job.recent,
                    &job.prefixes,
                    &job.query,
                );
                let targets: Vec<String> = results
                    .rows
                    .iter()
                    .filter_map(|row| row.target())
                    .filter(|target| !launcher::is_web_url(target))
                    .map(str::to_owned)
                    .collect();
                if done.send(SearchDone::Results(job.id, results)).is_err() {
                    break;
                }
                ctx.request_repaint();
                for target in targets {
                    if let Ok(newer) = jobs.try_recv() {
                        pending = Some(newer);
                        break;
                    }
                    if !icons.contains_key(&target) {
                        if icons.len() >= 256 {
                            icons.clear();
                        }
                        #[cfg(windows)]
                        let rgba = if launcher::is_app_reference(&target) {
                            launcher::windows::apps_folder_icon(&target)
                        } else {
                            launcher::windows::icon(Path::new(&target))
                        };
                        #[cfg(not(windows))]
                        let rgba = None;
                        icons.insert(target.clone(), rgba);
                    }
                    if let Some(rgba) = icons.get(&target).cloned().flatten() {
                        if done.send(SearchDone::Icon(target, rgba)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            }
        });
        Self { tx, rx }
    }
}

#[derive(Default)]
pub(super) struct LauncherPage {
    pub active: bool,
    pub focus: bool,
    query: String,
    catalog: Arc<Catalog>,
    scan: Option<mpsc::Receiver<(Catalog, bool)>>,
    rescan: bool,
    worker: Option<Worker>,
    loaded: bool,
    generation: u64,
    pending: bool,
    dirty: bool,
    rows: Vec<Row>,
    selected: usize,
    manager: bool,
    editor: Option<Entry>,
    editing: Option<usize>,
    roots: String,
    bookmarks: bool,
    notice: String,
    search_note: String,
    confirm: Option<Row>,
    icons: HashMap<String, egui::TextureHandle>,
    original: Option<egui::Rect>,
    work: Option<egui::Rect>,
    size: Option<Vec2>,
}
impl LauncherPage {
    pub(super) fn initial() -> Self {
        let mut page = Self {
            active: std::env::args().any(|a| a == "--launcher"),
            ..Self::default()
        };
        if std::env::var_os("CLIBO_DATA_DIR").is_some() && std::env::args().any(|a| a == "--smoke")
        {
            page.query = std::env::var("CLIBO_LAUNCHER_QUERY").unwrap_or_default();
        }
        page
    }
    pub(super) fn reopen(&mut self) {
        self.focus = true;
        self.query.clear();
        self.selected = 0;
        self.manager = false;
        self.editor = None;
        self.confirm = None;
        self.notice.clear();
        self.dirty = true;
    }
    pub(super) fn managing(&self) -> bool {
        self.manager || self.editor.is_some() || self.confirm.is_some()
    }
    pub(super) fn settings_changed(&mut self, scope_changed: bool, rules_changed: bool) {
        if scope_changed {
            self.rescan = true;
        }
        if rules_changed {
            self.dirty = true;
        }
    }
    pub(super) fn enter_window(
        &mut self,
        ctx: &egui::Context,
        owner: usize,
        original: Option<egui::Rect>,
    ) {
        if self.original.is_some() {
            return;
        }
        self.original = original.or_else(|| ctx.input(|i| i.viewport().inner_rect));
        let mut work = ctx.input(|i| {
            egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                i.viewport().monitor_size.unwrap_or(Vec2::new(1920., 1080.)),
            )
        });
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            };
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(
                MonitorFromWindow(owner as _, MONITOR_DEFAULTTONEAREST),
                &mut info,
            ) != 0
            {
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
        #[cfg(not(windows))]
        let _ = owner;
        self.work = Some(work);
        self.size = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(Vec2::new(400., 130.)));
        ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
    }
    fn fit(&mut self, ctx: &egui::Context) {
        if self.pending && !self.managing() && self.size.is_some() {
            return;
        }
        let Some(work) = self.work else {
            return;
        };
        let extra = if self.notice.is_empty() && self.search_note.is_empty() {
            0.
        } else {
            34.
        };
        let height = if self.managing() {
            610.
        } else {
            156. + self.rows.len().clamp(1, 7) as f32 * 62. + extra
        };
        let size = Vec2::new(740., height).min(work.size() - Vec2::splat(24.));
        if self.size != Some(size) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            if self.size.is_none() {
                let min = egui::pos2(
                    work.center().x - size.x / 2.,
                    work.top() + (work.height() - size.y) * 0.32,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(min));
            }
            self.size = Some(size);
        }
    }
    pub(super) fn restore_window(&mut self, ctx: &egui::Context) -> bool {
        let Some(original) = self.original.take() else {
            return false;
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(Vec2::new(560., 440.)));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(original.size()));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(original.min));
        self.size = None;
        self.work = None;
        true
    }
}

#[derive(Clone, Copy, Default)]
struct Keys {
    down: bool,
    up: bool,
    enter: bool,
    folder: bool,
    admin: bool,
    escape: bool,
}
fn launcher_keys(ctx: &egui::Context, composing: &mut bool, blocked: bool) -> Keys {
    let mut ime = false;
    ctx.input(|input| {
        for event in &input.events {
            if let egui::Event::Ime(event) = event {
                ime = true;
                match event {
                    egui::ImeEvent::Preedit(text) => *composing = !text.is_empty(),
                    egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => *composing = false,
                    _ => {}
                }
            }
        }
    });
    if ime || *composing || egui::Popup::is_any_open(ctx) {
        return Keys::default();
    }
    ctx.input_mut(|input| {
        let escape = input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
        if blocked {
            return Keys {
                escape,
                ..Keys::default()
            };
        }
        Keys {
            escape,
            down: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            up: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                || input.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
            admin: input.consume_key(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Enter,
            ),
            folder: input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter),
            enter: input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        }
    })
}

impl App {
    fn scan_launcher(&mut self, ctx: &egui::Context) {
        if self.launcher.scan.is_some() {
            self.launcher.rescan = true;
            return;
        }
        let settings = self.backend.store.lock().unwrap().settings.clone();
        let (tx, rx) = mpsc::channel();
        self.launcher.scan = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let catalog = launcher::catalog::build(
                &settings.launcher_roots,
                settings.launcher_bookmarks,
                |apps| {
                    let _ = tx.send((apps, false));
                    ctx.request_repaint();
                },
            );
            let _ = tx.send((catalog, true));
            ctx.request_repaint();
        });
    }
    fn persist_launcher(&mut self, edit: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut store = self.backend.store.lock().unwrap();
        let mut next = store.settings.clone();
        edit(&mut next);
        store.save_settings(next.clone())?;
        self.settings.launcher_entries = next.launcher_entries;
        self.settings.launcher_recent = next.launcher_recent;
        self.settings.launcher_roots = next.launcher_roots;
        self.settings.launcher_bookmarks = next.launcher_bookmarks;
        self.launcher.dirty = true;
        Ok(())
    }
    fn request_launcher_search(&mut self) {
        if !self.launcher.dirty {
            return;
        }
        let settings = self.backend.store.lock().unwrap().settings.clone();
        self.launcher.generation = self.launcher.generation.wrapping_add(1);
        self.launcher.pending = true;
        self.launcher.dirty = false;
        // Stale results cannot be launched while the new query is in flight.
        self.launcher.rows.clear();
        self.launcher.selected = 0;
        self.launcher.search_note.clear();
        if let Some(worker) = &self.launcher.worker {
            let _ = worker.tx.send(SearchJob {
                id: self.launcher.generation,
                catalog: self.launcher.catalog.clone(),
                custom: settings.launcher_entries,
                recent: settings.launcher_recent,
                prefixes: settings.launcher_search_prefixes,
                query: self.launcher.query.clone(),
            });
        }
    }
    fn launcher_manage(&mut self) {
        self.launcher.manager = !self.launcher.manager;
        if self.launcher.manager {
            let settings = &self.backend.store.lock().unwrap().settings;
            self.launcher.roots = settings.launcher_roots.join("\n");
            self.launcher.bookmarks = settings.launcher_bookmarks;
        } else {
            self.launcher.focus = true;
        }
    }
    fn leave_launcher(&mut self) {
        self.launcher.active = false;
        self.focus_search = true;
        self.dirty = true;
    }
    fn launcher_execute(
        &mut self,
        ctx: &egui::Context,
        row: Row,
        folder: bool,
        admin: bool,
        confirmed: bool,
    ) {
        if folder {
            let result = row
                .target()
                .filter(|target| !launcher::is_web_url(target))
                .and_then(|target| Path::new(target).parent())
                .ok_or_else(|| "此结果没有所在文件夹".to_owned())
                .and_then(|path| {
                    launcher::launch(&Entry {
                        name: "所在文件夹".into(),
                        target: path.to_string_lossy().into_owned(),
                    })
                });
            match result {
                Ok(()) => self.hide(ctx),
                Err(error) => self.launcher.notice = error,
            }
            return;
        }
        let result = match row.action.clone() {
            Action::Open(entry) => {
                let result = launcher::launch_as(&entry, admin);
                if result.is_ok() {
                    let target = entry.target;
                    if let Err(error) = self.persist_launcher(|s| {
                        s.launcher_recent.retain(|old| old != &target);
                        s.launcher_recent.insert(0, target);
                        s.launcher_recent.truncate(50);
                    }) {
                        self.backend.report(Err(error));
                    }
                }
                result
            }
            Action::Copy(text) => platform::write(Some(&text), None, self.owner),
            Action::Builtin(action) => {
                if action.requires_confirmation() && !confirmed {
                    self.launcher.confirm = Some(row);
                    return;
                }
                match action {
                    Builtin::Clipboard => {
                        self.leave_launcher();
                        return;
                    }
                    Builtin::Json => {
                        self.leave_launcher();
                        self.json.active = true;
                        return;
                    }
                    Builtin::Timestamp => {
                        self.leave_launcher();
                        self.timestamp.active = true;
                        return;
                    }
                    Builtin::Preferences => {
                        self.leave_launcher();
                        self.open_settings(SettingsSection::Launcher);
                        return;
                    }
                    Builtin::Refresh => {
                        self.scan_launcher(ctx);
                        return;
                    }
                    _ => launcher::system_action(action),
                }
            }
        };
        match result {
            Ok(()) => self.hide(ctx),
            Err(error) => self.launcher.notice = error,
        }
    }
    pub(super) fn launcher_page(&mut self, ctx: &egui::Context) {
        if !self.launcher.loaded {
            self.launcher.loaded = true;
            self.launcher.focus = true;
            self.launcher.dirty = true;
            self.launcher.worker = Some(Worker::new(ctx));
            self.scan_launcher(ctx);
        }
        if let Some(rx) = &self.launcher.scan {
            match rx.try_recv() {
                Ok((catalog, complete)) => {
                    self.launcher.catalog = Arc::new(catalog);
                    if complete {
                        self.launcher.scan = None;
                    }
                    self.launcher.dirty = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.launcher.scan = None;
                    self.launcher.notice = "扫描未完成，请按 F5 重试".into();
                }
                _ => {}
            }
        }
        if self.launcher.scan.is_none() && std::mem::take(&mut self.launcher.rescan) {
            self.scan_launcher(ctx);
        }
        if let Some(worker) = &self.launcher.worker {
            while let Ok(done) = worker.rx.try_recv() {
                match done {
                    SearchDone::Results(id, results)
                        if id == self.launcher.generation && !self.launcher.dirty =>
                    {
                        self.launcher.icons.retain(|target, _| {
                            results
                                .rows
                                .iter()
                                .any(|row| row.target() == Some(target.as_str()))
                        });
                        self.launcher.rows = results.rows;
                        self.launcher.search_note = results.note;
                        self.launcher.pending = false;
                    }
                    SearchDone::Icon(target, rgba)
                        if self
                            .launcher
                            .rows
                            .iter()
                            .any(|row| row.target() == Some(target.as_str())) =>
                    {
                        self.launcher.icons.insert(
                            target.clone(),
                            ctx.load_texture(
                                format!("launcher-icon-{target}"),
                                egui::ColorImage::from_rgba_premultiplied([32, 32], &rgba),
                                egui::TextureOptions::LINEAR,
                            ),
                        );
                    }
                    _ => {}
                }
            }
        }
        let keys = launcher_keys(ctx, &mut self.ime_composing, self.launcher.managing());
        if keys.escape {
            if self.launcher.confirm.take().is_some() || self.launcher.editor.take().is_some() {
                self.launcher.focus = true;
            } else if self.launcher.manager {
                self.launcher.manager = false;
                self.launcher.focus = true;
            } else {
                self.hide(ctx);
                return;
            }
        }
        if !self.ime_composing
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5))
        {
            self.scan_launcher(ctx);
        }
        if !self.ime_composing
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::I))
        {
            self.launcher_manage();
        }
        // 与剪贴板面板共用「精密仪器」双主题，由设置统一管理深浅色。
        let p = palette(self.dark, self.theme);
        let bg = p.panel;
        let text = p.text;
        let muted = p.text_dim;
        let hover = p.card_hover;
        let selected = p.card_selected;
        let accent = p.accent;
        let border = p.hairline;
        let mut execute = None;
        let mut copy = None;
        let mut remove = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(12),
            )
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(text);
                ui.visuals_mut().selection.bg_fill = p.accent_soft;
                ui.visuals_mut().selection.stroke.color = text;
                ui.visuals_mut().widgets.inactive.bg_fill = bg;
                ui.visuals_mut().widgets.hovered.bg_fill = hover;
                ui.visuals_mut().widgets.active.bg_fill = hover;
                ui.spacing_mut().item_spacing = Vec2::new(8., 6.);
                let prefixes = self
                    .backend
                    .store
                    .lock()
                    .unwrap()
                    .settings
                    .launcher_search_prefixes
                    .clone();
                let lower_query = self.launcher.query.trim_start().to_lowercase();
                let everything_active = self.launcher.catalog.everything.is_some()
                    && lower_query.split_once(' ').is_some_and(|(prefix, query)| {
                        prefix == prefixes.files.to_lowercase() && !query.trim().is_empty()
                    });
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(30., 48.), egui::Sense::hover());
                    let center = rect.center() - Vec2::new(3., 3.);
                    ui.painter()
                        .circle_stroke(center, 7., egui::Stroke::new(2., accent));
                    ui.painter().line_segment(
                        [center + Vec2::splat(5.), center + Vec2::splat(11.)],
                        egui::Stroke::new(2., accent),
                    );
                    let response = ui.add_sized(
                        Vec2::new(ui.available_width() - 104., 48.),
                        egui::TextEdit::singleline(&mut self.launcher.query)
                            .id(egui::Id::new("launcher-search"))
                            .font(egui::FontId::proportional(25.))
                            .frame(false)
                            .hint_text("搜索应用、文件、书签…")
                            .vertical_align(egui::Align::Center),
                    );
                    if self.launcher.focus && !self.launcher.managing() {
                        response.request_focus();
                        self.launcher.focus = false;
                    }
                    if response.changed() {
                        self.launcher.dirty = true;
                        self.launcher.notice.clear();
                    }
                    let (everything_rect, everything_response) =
                        ui.allocate_exact_size(Vec2::new(22., 22.), egui::Sense::hover());
                    if everything_active {
                        ui.painter()
                            .rect_filled(everything_rect, CornerRadius::same(6), accent);
                        ui.painter().text(
                            everything_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "E",
                            egui::FontId::proportional(12.),
                            bg,
                        );
                        everything_response.on_hover_text("Everything 全盘搜索已启用");
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("⚙").size(21.).color(muted))
                                .frame(false),
                        )
                        .on_hover_text("管理入口与搜索范围 · Ctrl+I")
                        .clicked()
                    {
                        self.launcher_manage();
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("×").size(23.).color(muted))
                                .frame(false),
                        )
                        .on_hover_text("收起 · Esc")
                        .clicked()
                    {
                        self.hide(ctx);
                    }
                });
                let line = ui.available_rect_before_wrap();
                ui.painter()
                    .hline(line.x_range(), line.top(), egui::Stroke::new(1., border));
                ui.add_space(9.);
                ui.horizontal(|ui| {
                    for (label, prefix) in [
                        ("全部", String::new()),
                        ("应用", format!("{} ", prefixes.apps)),
                        ("文件", format!("{} ", prefixes.files)),
                        ("书签", format!("{} ", prefixes.bookmarks)),
                        ("系统", format!("{} ", prefixes.system)),
                    ] {
                        let key = prefix.trim_end().to_lowercase();
                        let active = if key.is_empty() {
                            lower_query.is_empty()
                        } else {
                            lower_query == key || lower_query.starts_with(&format!("{key} "))
                        };
                        if pill(ui, &p, label, active).clicked() {
                            self.launcher.query = prefix;
                            self.launcher.focus = true;
                            self.launcher.dirty = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.launcher.scan.is_some() || self.launcher.pending {
                            ui.spinner();
                        }
                        if ghost(ui, "← 剪贴板").on_hover_text("返回剪贴板").clicked() {
                            self.leave_launcher();
                        }
                    });
                });
                if !self.hotkey_error.is_empty() {
                    ui.colored_label(p.error, "快捷键被其他程序占用，可在 Clibo 偏好设置中修改")
                        .on_hover_text(&self.hotkey_error);
                }
                if !self.launcher.notice.is_empty() {
                    ui.label(RichText::new(&self.launcher.notice).size(12.).color(accent));
                }
                if !self.launcher.search_note.is_empty() {
                    ui.label(
                        RichText::new(&self.launcher.search_note)
                            .size(12.)
                            .color(muted),
                    );
                }
                if let Some(row) = self.launcher.confirm.clone() {
                    ui.add_space(20.);
                    ui.heading(format!("确认{}？", row.title));
                    ui.label("请先保存其他应用中的工作。");
                    ui.horizontal(|ui| {
                        if ui.button("确认执行").clicked() {
                            self.launcher.confirm = None;
                            execute = Some((row, false, false, true));
                        }
                        if ui.button("取消").clicked() {
                            self.launcher.confirm = None;
                            self.launcher.focus = true;
                        }
                    });
                } else if self.launcher.manager || self.launcher.editor.is_some() {
                    self.launcher_manager_ui(ui, ctx);
                } else {
                    // Apply current input before handling Enter, so results for an old query can never run.
                    self.request_launcher_search();
                    let rows = self.launcher.rows.clone();
                    if !rows.is_empty() {
                        self.launcher.selected = self.launcher.selected.min(rows.len() - 1);
                        if keys.down {
                            self.launcher.selected = (self.launcher.selected + 1) % rows.len();
                        }
                        if keys.up {
                            self.launcher.selected =
                                (self.launcher.selected + rows.len() - 1) % rows.len();
                        }
                        if keys.enter || keys.folder || keys.admin {
                            execute = Some((
                                rows[self.launcher.selected].clone(),
                                keys.folder,
                                keys.admin,
                                false,
                            ));
                        }
                    }
                    let max_height = (ui.available_height() - 31.).max(60.);
                    egui::ScrollArea::vertical()
                        .id_salt("launcher-results")
                        .max_height(max_height)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.;
                            for (index, row) in rows.iter().enumerate() {
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), 60.),
                                    egui::Sense::click(),
                                );
                                let active = index == self.launcher.selected;
                                if active {
                                    ui.painter()
                                        .rect_filled(rect, CornerRadius::same(8), selected);
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(
                                            rect.min + Vec2::new(1.5, 1.5),
                                            Vec2::new(2.5, rect.height() - 3.),
                                        ),
                                        CornerRadius::same(1),
                                        accent,
                                    );
                                } else if response.hovered() {
                                    ui.painter().rect_filled(rect, CornerRadius::same(8), hover);
                                }
                                let icon_rect = egui::Rect::from_min_size(
                                    rect.min + Vec2::new(15., 14.),
                                    Vec2::splat(32.),
                                );
                                if let Some(texture) = row
                                    .target()
                                    .and_then(|target| self.launcher.icons.get(target))
                                {
                                    ui.painter().image(
                                        texture.id(),
                                        icon_rect,
                                        egui::Rect::from_min_max(
                                            egui::Pos2::ZERO,
                                            egui::pos2(1., 1.),
                                        ),
                                        Color32::WHITE,
                                    );
                                } else {
                                    ui.painter().rect_filled(
                                        icon_rect,
                                        CornerRadius::same(7),
                                        p.accent_soft,
                                    );
                                    ui.painter().text(
                                        icon_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        row.kind.glyph(),
                                        egui::FontId::proportional(21.),
                                        accent,
                                    );
                                }
                                let width = (rect.width() - 150.).max(30.);
                                let title = ui.painter().layout(
                                    row.title.clone(),
                                    egui::FontId::proportional(16.),
                                    text,
                                    f32::INFINITY,
                                );
                                let sub = ui.painter().layout(
                                    row.subtitle.clone(),
                                    egui::FontId::proportional(11.),
                                    muted,
                                    f32::INFINITY,
                                );
                                let clip = egui::Rect::from_min_size(
                                    rect.min + Vec2::new(61., 0.),
                                    Vec2::new(width, 60.),
                                );
                                ui.painter().with_clip_rect(clip).galley(
                                    rect.min + Vec2::new(61., 10.),
                                    title,
                                    text,
                                );
                                ui.painter().with_clip_rect(clip).galley(
                                    rect.min + Vec2::new(61., 34.),
                                    sub,
                                    muted,
                                );
                                ui.painter().text(
                                    egui::pos2(rect.right() - 14., rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    if active {
                                        "⏎ 打开"
                                    } else {
                                        row.kind.label()
                                    },
                                    egui::FontId::proportional(12.),
                                    if active { accent } else { muted },
                                );
                                if response.clicked() {
                                    self.launcher.selected = index;
                                    execute = Some((row.clone(), false, false, false));
                                }
                                if active && (keys.down || keys.up) {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                response
                                    .on_hover_ui(|ui| {
                                        ui.set_min_width(430.);
                                        ui.set_max_width(520.);
                                        ui.horizontal_top(|ui| {
                                            if let Some(texture) = row
                                                .target()
                                                .and_then(|target| self.launcher.icons.get(target))
                                            {
                                                ui.image((texture.id(), Vec2::splat(56.)));
                                            } else {
                                                let (icon_rect, _) = ui.allocate_exact_size(
                                                    Vec2::splat(56.),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    icon_rect,
                                                    CornerRadius::same(10),
                                                    p.accent_soft,
                                                );
                                                ui.painter().text(
                                                    icon_rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    row.kind.glyph(),
                                                    egui::FontId::proportional(26.),
                                                    accent,
                                                );
                                            }
                                            ui.vertical(|ui| {
                                                let text_width = 340.;
                                                ui.add_sized(
                                                    Vec2::new(text_width, 22.),
                                                    egui::Label::new(
                                                        RichText::new(&row.title).strong(),
                                                    )
                                                    .truncate(),
                                                );
                                                ui.add_sized(
                                                    Vec2::new(text_width, 18.),
                                                    egui::Label::new(
                                                        RichText::new(&row.subtitle)
                                                            .size(11.)
                                                            .color(muted),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(&row.subtitle);
                                            });
                                        });
                                    })
                                    .context_menu(|ui| {
                                        if ui.button("打开 / 执行").clicked() {
                                            execute = Some((row.clone(), false, false, false));
                                            ui.close();
                                        }
                                        if let Some(target) = row.target() {
                                            if ui.button("复制路径 / 网址").clicked() {
                                                copy = Some(target.to_owned());
                                                ui.close();
                                            }
                                            if !launcher::is_web_url(target) {
                                                if ui
                                                    .button("打开所在文件夹 · Ctrl+Enter")
                                                    .clicked()
                                                {
                                                    execute =
                                                        Some((row.clone(), true, false, false));
                                                    ui.close();
                                                }
                                                #[cfg(windows)]
                                                if ui
                                                    .button("以管理员身份打开 · Ctrl+Shift+Enter")
                                                    .clicked()
                                                {
                                                    execute =
                                                        Some((row.clone(), false, true, false));
                                                    ui.close();
                                                }
                                            }
                                            if let Some(index) = row.custom {
                                                if ui.button("编辑入口").clicked() {
                                                    self.launcher.editor = Some(Entry {
                                                        name: row.title.clone(),
                                                        target: target.into(),
                                                    });
                                                    self.launcher.editing = Some(index);
                                                    ui.close();
                                                }
                                                if ui.button("删除自定义入口").clicked() {
                                                    remove = Some(index);
                                                    ui.close();
                                                }
                                            }
                                        }
                                    });
                            }
                            if rows.is_empty() {
                                ui.add_space(24.);
                                ui.label(
                                    RichText::new(if self.launcher.pending {
                                        "正在搜索…"
                                    } else {
                                        "没有匹配结果"
                                    })
                                    .size(16.)
                                    .color(muted),
                                );
                                ui.label(
                                    RichText::new("试试 = 12*3、g 关键词，或输入完整文件路径")
                                        .size(12.)
                                        .color(muted),
                                );
                                ui.add_space(20.);
                            }
                        });
                }
                ui.add_space(5.);
                let footer_top = ui.cursor().top();
                ui.painter().hline(
                    ui.available_rect_before_wrap().x_range(),
                    footer_top,
                    egui::Stroke::new(1., border),
                );
                ui.add_space(7.);
                ui.horizontal(|ui| {
                    ui.label(mono("●").size(9.).color(accent));
                    ui.label(
                        RichText::new(format!(
                            "{} · {} 个结果",
                            pretty_hotkey(&self.settings.hotkeys.launcher),
                            self.launcher.rows.len()
                        ))
                        .size(10.)
                        .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.;
                        for (key, label) in [
                            ("Esc", "关闭"),
                            ("F5", "刷新"),
                            ("⏎", "打开"),
                            ("↑↓", "选择"),
                        ] {
                            ui.label(RichText::new(label).size(10.).color(muted));
                            kbd(ui, &p, key);
                            ui.add_space(6.);
                        }
                    });
                });
            });
        if let Some(text) = copy {
            if let Err(error) = platform::write(Some(&text), None, self.owner) {
                self.launcher.notice = error;
            }
        }
        if let Some(index) = remove {
            if let Err(error) = self.persist_launcher(|s| {
                if index < s.launcher_entries.len() {
                    s.launcher_entries.remove(index);
                }
            }) {
                self.launcher.notice = error;
            }
        }
        if let Some((row, folder, admin, confirmed)) = execute {
            self.launcher_execute(ctx, row, folder, admin, confirmed);
        }
        self.request_launcher_search();
        self.launcher.fit(ctx);
    }
    fn launcher_manager_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().id_salt("launcher-manager").max_height((ui.available_height()-32.).max(100.)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("管理启动器");
                if ui.button("返回搜索").clicked() { self.launcher.manager = false; self.launcher.editor = None; self.launcher.focus = true; }
                if ui.button("＋ 添加入口").clicked() { self.launcher.editor = Some(Entry { name: String::new(), target: String::new() }); self.launcher.editing = None; }
            });
            if let Some(mut entry) = self.launcher.editor.clone() {
                ui.add(egui::TextEdit::singleline(&mut entry.name).hint_text("入口名称").desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(&mut entry.target).hint_text("完整路径或 https:// 网址").desired_width(f32::INFINITY));
                self.launcher.editor = Some(entry.clone());
                ui.horizontal(|ui| {
                    if ui.button("保存入口").clicked() {
                        entry.name = entry.name.trim().into(); entry.target = launcher::search::expand_path(&entry.target);
                        let index = self.launcher.editing;
                        let result = entry.validate().and_then(|()| self.persist_launcher(|s| {
                            if let Some(old) = index.and_then(|i| s.launcher_entries.get_mut(i)) { *old = entry.clone(); }
                            else { s.launcher_entries.push(entry.clone()); }
                        }));
                        match result { Ok(()) => { self.launcher.editor = None; self.launcher.notice = "入口已保存".into(); }, Err(error) => self.launcher.notice = error }
                    }
                    if ui.button("取消").clicked() { self.launcher.editor = None; }
                });
            }
            ui.separator();
            ui.label("搜索目录（每行一个；留空使用桌面、文档、下载）");
            ui.add(egui::TextEdit::multiline(&mut self.launcher.roots).desired_rows(3).desired_width(f32::INFINITY));
            ui.checkbox(&mut self.launcher.bookmarks, "搜索浏览器书签（Chrome / Edge / Brave / Firefox）");
            if ui.button("保存范围并重新索引").clicked() {
                let roots: Vec<String> = self.launcher.roots.lines().map(launcher::search::expand_path).filter(|s| !s.is_empty()).collect();
                if roots.iter().any(|root| !Path::new(root).is_absolute() || !Path::new(root).is_dir()) {
                    self.launcher.notice = "搜索目录必须是存在的绝对路径".into();
                } else {
                    let bookmarks = self.launcher.bookmarks;
                    match self.persist_launcher(|s| { s.launcher_roots = roots; s.launcher_bookmarks = bookmarks; }) {
                        Ok(()) => { self.launcher.notice = "搜索范围已保存".into(); self.scan_launcher(ctx); }
                        Err(error) => self.launcher.notice = error,
                    }
                }
            }
            ui.small(format!("已索引 {} 项；刷新后加载新应用、文件和书签。", self.launcher.catalog.items.len()));
            let prefixes = self.backend.store.lock().unwrap().settings.launcher_search_prefixes.clone();
            ui.small(if self.launcher.catalog.everything.is_some() {
                format!("Everything 已接入：{} 关键词将同时查询 Everything。", prefixes.files)
            } else {
                format!("文件搜索使用本地目录索引。安装 Everything 及 es.exe（加入 PATH）后可用 {} 查询全盘索引。", prefixes.files)
            });
            for note in &self.launcher.catalog.notes { ui.small(note); }
            ui.separator();
            ui.label("自定义入口");
            let entries = self.backend.store.lock().unwrap().settings.launcher_entries.clone();
            for (index, entry) in entries.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&entry.name);
                    if ui.small_button("编辑").clicked() { self.launcher.editor = Some(entry.clone()); self.launcher.editing = Some(index); }
                    if ui.small_button("删除").clicked() {
                        match self.persist_launcher(|s| { s.launcher_entries.remove(index); }) {
                            Ok(()) => { self.launcher.editor = None; }
                            Err(error) => self.launcher.notice = error,
                        }
                    }
                });
            }
            ui.separator();
            ui.label(format!(
                "关键词：{} 应用 · {} 文件 · {} 书签 · {} 系统",
                prefixes.apps, prefixes.files, prefixes.bookmarks, prefixes.system
            ));
            ui.label(format!(
                "网页：{} Google · {} Bing · {} 百度 · 计算：{} 算式",
                prefixes.google, prefixes.bing, prefixes.baidu, prefixes.calculator
            ));
            ui.small("搜索指令和快捷键可在 Clibo 偏好设置 → 启动器 / 快捷键中修改。");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ime_commit_does_not_launch_and_modifier_actions_are_distinct() {
        let ctx = egui::Context::default();
        let key = |modifiers| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let input = egui::RawInput {
            events: vec![
                egui::Event::Ime(egui::ImeEvent::Commit("测试".into())),
                key(egui::Modifiers::NONE),
            ],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            assert!(!launcher_keys(ctx, &mut true, false).enter);
        });
        let input = egui::RawInput {
            events: vec![key(egui::Modifiers::CTRL)],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            let keys = launcher_keys(ctx, &mut false, false);
            assert!(keys.folder);
            assert!(!keys.enter);
            assert!(!keys.admin);
        });
    }
    #[test]
    fn window_restore_is_once_and_uses_original_geometry() {
        let ctx = egui::Context::default();
        let original = egui::Rect::from_min_size(egui::pos2(200., 100.), Vec2::new(780., 580.));
        let mut page = LauncherPage::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            page.enter_window(ctx, 0, Some(original));
            assert_eq!(page.original, Some(original));
            page.enter_window(ctx, 0, None);
            assert_eq!(page.original, Some(original));
            assert!(page.restore_window(ctx));
            assert!(!page.restore_window(ctx));
        });
    }
}
