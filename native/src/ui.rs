use crate::{
    backend::{self, Backend},
    model::*,
    platform,
    store::{ImageReader, Store},
};
use eframe::egui::{self, Color32, CornerRadius, RichText, Vec2};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers as HotkeyMods},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{atomic::Ordering, mpsc, Arc},
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIcon, TrayIconBuilder,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F10,
    VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F2, VK_F20, VK_F21,
    VK_F22, VK_F23, VK_F24, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_INSERT,
    VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4,
    VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_OEM_1, VK_OEM_2, VK_OEM_3,
    VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS,
    VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

mod calculator_ui;
mod json_parser;
mod json_ui;
mod json_view;
mod launcher_ui;
mod timestamp_ui;

const ROW_HEIGHT: f32 = 64.;
const ROW_STEP: f32 = ROW_HEIGHT + 8.;
/// Bounded row-thumbnail texture cache (24 × ~320×240 ≈ 7 MB of GPU memory).
const THUMB_CACHE: usize = 24;
/// Maximum row thumbnail jobs queued per frame; decoding happens off the UI thread.
const THUMB_BUDGET: u8 = 4;
const APP_CORNER_RADIUS: u8 = 14;

enum Event {
    Open(bool, usize, Option<String>),
    OpenJson,
    OpenTimestamp,
    OpenLauncher,
    OpenSettings,
    LauncherHotkey(bool),
    Quit,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ImageKind {
    Thumbnail,
    Full(u32),
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ImageJob {
    id: String,
    kind: ImageKind,
    epoch: u64,
}

struct DecodedImage {
    size: [usize; 2],
    rgba: Vec<u8>,
}

struct ImageDone {
    job: ImageJob,
    result: Result<DecodedImage, String>,
}

fn decode_image(png: &[u8], max_side: Option<u32>) -> Result<DecodedImage, String> {
    let mut image = image::load_from_memory(png).map_err(|e| e.to_string())?;
    if let Some(side) = max_side {
        if image.width() == 0
            || image.height() == 0
            || u64::from(image.width()) * u64::from(image.height()) > 25_000_000
        {
            return Err("图片超过预览大小限制".into());
        }
        if image.width() > side || image.height() > side {
            image = image.thumbnail(side, side);
        }
    }
    let rgba = image.into_rgba8();
    Ok(DecodedImage {
        size: [rgba.width() as usize, rgba.height() as usize],
        rgba: rgba.into_raw(),
    })
}

fn spawn_image_worker(
    path: PathBuf,
    ctx: egui::Context,
) -> (mpsc::SyncSender<ImageJob>, mpsc::Receiver<ImageDone>) {
    let (job_tx, job_rx) = mpsc::sync_channel::<ImageJob>(32);
    let (done_tx, done_rx) = mpsc::channel::<ImageDone>();
    std::thread::spawn(move || {
        let reader = ImageReader::open(&path);
        while let Ok(job) = job_rx.recv() {
            let result = match &reader {
                Ok(reader) => match &job.kind {
                    ImageKind::Thumbnail => reader
                        .thumbnail(&job.id)
                        .and_then(|png| decode_image(&png, None)),
                    ImageKind::Full(side) => reader
                        .image(&job.id)
                        .and_then(|png| decode_image(&png, Some(*side))),
                },
                Err(error) => Err(error.clone()),
            };
            if done_tx.send(ImageDone { job, result }).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (job_tx, done_rx)
}
fn dispatch(tx: &mpsc::Sender<Event>, event: Event, backend: &Backend, ctx: &egui::Context) {
    // Capture before revealing/focusing Clibo. On macOS the clipboard source is
    // inferred from the foreground app, and on every platform this closes the
    // race with the asynchronous clipboard listener.
    let event = match event {
        Event::Open(queue, target, _) => {
            let clipboard = match backend.capture_current() {
                Ok(id) => id,
                Err(error) => {
                    backend.report(Err(error));
                    None
                }
            };
            Event::Open(queue, target, clipboard)
        }
        event => event,
    };
    if tx.send(event).is_ok() {
        // A hidden native window may never redraw from request_repaint alone.
        // Every command, including Quit, must wake it before update can run.
        crate::visibility::reveal(backend.window.load(Ordering::Relaxed));
        ctx.request_repaint();
    }
}
/// Which hotkey field the settings dialog is currently recording.
#[derive(Clone, Copy, PartialEq)]
enum HotkeySlot {
    Show,
    Queue,
    Find,
    Launcher,
    Json,
    Timestamp,
}
/// `>` 命令模式里的一个工具条目。
struct CommandItem {
    id: &'static str,
    icon: &'static str,
    name: &'static str,
    desc: &'static str,
    meta: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    General,
    Clipboard,
    Launcher,
    Hotkeys,
    Data,
}
impl SettingsSection {
    const ALL: [SettingsSection; 5] = [
        SettingsSection::General,
        SettingsSection::Clipboard,
        SettingsSection::Launcher,
        SettingsSection::Hotkeys,
        SettingsSection::Data,
    ];
    fn label(self) -> &'static str {
        match self {
            SettingsSection::General => "通用",
            SettingsSection::Clipboard => "剪贴板",
            SettingsSection::Launcher => "启动器",
            SettingsSection::Hotkeys => "快捷键",
            SettingsSection::Data => "数据与备份",
        }
    }
    /// Keywords matched by the in-settings search box.
    fn keywords(self) -> &'static str {
        match self {
            SettingsSection::General => {
                "通用 开机自启 外观 主题 浅色 深色 蓝灰 琥珀 记录剪贴板 暂停"
            }
            SettingsSection::Clipboard => {
                "剪贴板 采集 记录图片 跳过疑似密钥 历史上限 保留天数 JSON 历史 排除应用 分组"
            }
            SettingsSection::Launcher => {
                "启动器 搜索范围 书签 搜索目录 搜索指令 前缀 应用 文件 网页 计算"
            }
            SettingsSection::Hotkeys => {
                "快捷键 呼出面板 排列粘贴 队列 启动器 查找 重录 JSON 时间戳"
            }
            SettingsSection::Data => "数据 备份 恢复 清理",
        }
    }
    fn matches(self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.label().contains(&query)
            || self
                .keywords()
                .split_whitespace()
                .any(|word| word.to_lowercase().contains(&query))
    }
}
pub struct App {
    backend: Arc<Backend>,
    _instance: crate::instance::Instance,
    _tray: Option<TrayIcon>,
    hotkeys: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    hotkey_error: String,
    hotkey_capture: Option<HotkeySlot>,
    capture_error: String,
    events: mpsc::Receiver<Event>,
    query: String,
    command_selected: usize,
    settings_query: String,
    calculator: crate::calculator::Calculator,
    json: json_ui::JsonPage,
    json_window: json_ui::WindowTransition,
    timestamp: timestamp_ui::TimestampPage,
    launcher: launcher_ui::LauncherPage,
    filter: String,
    group: String,
    items: Vec<ClipView>,
    selected: String,
    revision: u64,
    dirty: bool,
    focus_search: bool,
    scroll_selected: bool,
    history_offset: f32,
    history_up_distance: f32,
    history_top: bool,
    target: usize,
    owner: usize,
    pinned: bool,
    was_focused: bool,
    settings_open: bool,
    settings_section: SettingsSection,
    #[cfg(windows)]
    autostart: Result<bool, String>,
    settings: Settings,
    excluded: String,
    launcher_roots: String,
    group_name: String,
    note: String,
    memberships: Vec<String>,
    preview_id: String,
    preview_text: String,
    texture: Option<egui::TextureHandle>,
    enlarged_image: Option<egui::TextureHandle>,
    image_actual_size: bool,
    transform: String,
    confirm: Option<String>,
    preview_open: bool,
    /// 预览抽屉的持久偏好（设置开关与 Ctrl+P 共用）；选中图片时不再强行覆盖它。
    preview_default: bool,
    ime_composing: bool,
    smoke: Option<std::time::Instant>,
    screenshot_requested: bool,
    dark: bool,
    theme: Theme,
    thumbs: HashMap<String, egui::TextureHandle>,
    thumb_order: VecDeque<String>,
    thumb_budget: u8,
    preview_is_image: bool,
    image_tx: mpsc::SyncSender<ImageJob>,
    image_rx: mpsc::Receiver<ImageDone>,
    image_pending: HashSet<ImageJob>,
    image_failed: HashSet<ImageJob>,
    image_epoch: u64,
}

fn hotkey_specs(settings: &Settings) -> Result<Vec<(&'static str, HotKey)>, String> {
    let mut keys = Vec::new();
    for (name, raw) in [
        ("呼出面板", &settings.hotkey),
        ("排列粘贴", &settings.queue_hotkey),
        ("启动器", &settings.launcher_hotkey),
    ] {
        let key = HotKey::from_str(raw).map_err(|e| format!("{name}快捷键格式错误：{e}"))?;
        if keys.iter().any(|(_, old)| *old == key) {
            return Err("全局快捷键不能重复".into());
        }
        keys.push((name, key));
    }
    Ok(keys)
}

fn register(manager: &GlobalHotKeyManager, settings: &Settings) -> Result<Vec<HotKey>, String> {
    let mut registered = Vec::new();
    for (name, key) in hotkey_specs(settings)? {
        if let Err(error) = manager.register(key) {
            for key in &registered {
                let _ = manager.unregister(*key);
            }
            return Err(format!("{name}快捷键冲突：{error}"));
        }
        registered.push(key);
    }
    Ok(registered)
}

// At startup a single occupied hotkey must not disable all other shortcuts.
fn register_available(manager: &GlobalHotKeyManager, settings: &Settings) -> (Vec<HotKey>, String) {
    let mut registered = Vec::new();
    let mut errors = Vec::new();
    for (name, raw) in [
        ("呼出面板", &settings.hotkey),
        ("排列粘贴", &settings.queue_hotkey),
        ("启动器", &settings.launcher_hotkey),
    ] {
        match HotKey::from_str(raw) {
            Ok(key) => match manager.register(key) {
                Ok(()) => registered.push(key),
                Err(error) => errors.push(format!("{name}快捷键 {raw} 被占用：{error}")),
            },
            Err(error) => errors.push(format!("{name}快捷键格式错误：{error}")),
        }
    }
    (registered, errors.join("\n"))
}
pub fn run() -> Result<(), String> {
    let path = backend::data_dir()?;
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    let ui_state = UiState::load(&path);
    let Some(instance) = crate::instance::Instance::acquire(&path)? else {
        return Ok(());
    };
    let history_path = path.join("history.db");
    let mut store = Store::open(&history_path)?;
    if std::env::args().any(|a| a == "--demo") {
        if std::env::var_os("CLIBO_DATA_DIR").is_none() || !store.entries.is_empty() {
            return Err("演示模式要求 CLIBO_DATA_DIR 指向空的独立测试目录".into());
        }
        let mut settings = store.settings.clone();
        settings.hotkey = "Ctrl+Alt+F11".into();
        settings.queue_hotkey = "Ctrl+Alt+F12".into();
        settings.enabled = false;
        store.save_settings(settings)?;
        store.group("", "TOOL", false)?;
        for text in ["SELECT 用户名称\nFROM accounts\nWHERE enabled = true;", "https://example.com/clibo", "欢迎使用 Clibo 原生版\n\n搜索、收藏和排列粘贴，全部在本地完成。\n这个窗口不依赖 WebView。"] {
            store.insert(Captured {text:Some(text.into()),png:None,source:"Clibo Demo".into()})?;
        }
        if let Some(image_path) = std::env::var_os("CLIBO_DEMO_IMAGE") {
            store.insert(Captured {
                text: None,
                png: Some(std::fs::read(image_path).map_err(|e| e.to_string())?),
                source: "Clibo Demo".into(),
            })?;
        }
    }
    let target = platform::foreground();
    let background = std::env::args().any(|a| a == "--background");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon({
                let image = image::load_from_memory(include_bytes!("../icons/clibo.png"))
                    .expect("bundled application icon must be valid")
                    .into_rgba8();
                egui::IconData {
                    width: image.width(),
                    height: image.height(),
                    rgba: image.into_raw(),
                }
            })
            .with_title("Clibo · Native")
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size(
                if std::env::args().any(|a| a == "--smoke")
                    && std::env::args().any(|a| a == "--small")
                {
                    [560., 440.]
                } else {
                    [660., 580.]
                },
            )
            .with_min_inner_size([560., 440.])
            .with_always_on_top(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Clibo Native",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx, ui_state.dark, ui_state.theme);
            let backend = Backend::new(store, cc.egui_ctx.clone());
            let (image_tx, image_rx) =
                spawn_image_worker(history_path.clone(), cc.egui_ctx.clone());
            let settings = backend.store.lock().unwrap().settings.clone();
            let hotkeys = GlobalHotKeyManager::new()?;
            // Isolated screenshot runs must not compete with the user's live shortcuts.
            let (registered, hotkey_error) = if std::env::args().any(|arg| arg == "--smoke")
                && std::env::var_os("CLIBO_DATA_DIR").is_some()
            {
                (Vec::new(), String::new())
            } else {
                register_available(&hotkeys, &settings)
            };
            let (tx, rx) = mpsc::channel();
            let open_tx = tx.clone();
            let open_backend = backend.clone();
            let open_ctx = cc.egui_ctx.clone();
            instance.listen(move || {
                dispatch(
                    &open_tx,
                    Event::Open(false, 0, None),
                    &open_backend,
                    &open_ctx,
                );
            });
            // Exercise the same dispatch as the tray while the window is hidden.
            // Restricted to explicitly isolated smoke-test data directories.
            if std::env::args().any(|a| a == "--smoke-exit")
                && std::env::var_os("CLIBO_DATA_DIR").is_some()
            {
                let exit_tx = tx.clone();
                let exit_backend = backend.clone();
                let exit_ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    dispatch(&exit_tx, Event::Quit, &exit_backend, &exit_ctx);
                });
            }
            let hot_tx = tx.clone();
            let ctx = cc.egui_ctx.clone();
            // Read current registered settings when dispatched, so editing hotkeys works.
            let event_backend = backend.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                if event.state == HotKeyState::Pressed {
                    let settings = event_backend.store.lock().unwrap().settings.clone();
                    if HotKey::from_str(&settings.launcher_hotkey)
                        .is_ok_and(|key| key.id() == event.id)
                    {
                        let foreground = platform::foreground();
                        let was_foreground = foreground != 0
                            && foreground == event_backend.window.load(Ordering::Relaxed)
                            && event_backend.visible.load(Ordering::Relaxed);
                        dispatch(
                            &hot_tx,
                            Event::LauncherHotkey(was_foreground),
                            &event_backend,
                            &ctx,
                        );
                        return;
                    }
                    let queue = HotKey::from_str(&settings.queue_hotkey)
                        .is_ok_and(|key| key.id() == event.id);
                    dispatch(
                        &hot_tx,
                        Event::Open(queue, platform::foreground(), None),
                        &event_backend,
                        &ctx,
                    );
                }
            }));
            let menu = Menu::new();
            let open = MenuItem::new("打开 Clibo", true, None);
            let queue = MenuItem::new("排列粘贴", true, None);
            let json = MenuItem::new("JSON 工作页", true, None);
            let timestamp = MenuItem::new("时间戳转换", true, None);
            let launcher = MenuItem::new("启动器", true, None);
            let launcher_id = launcher.id().clone();
            let settings_item = MenuItem::new("设置", true, None);
            let settings_id = settings_item.id().clone();
            let quit = MenuItem::new("退出", true, None);
            menu.append_items(&[
                &open,
                &queue,
                &json,
                &timestamp,
                &launcher,
                &settings_item,
                &quit,
            ])?;
            let open_id = open.id().clone();
            let queue_id = queue.id().clone();
            let json_id = json.id().clone();
            let timestamp_id = timestamp.id().clone();
            let quit_id = quit.id().clone();
            let ctx = cc.egui_ctx.clone();
            let menu_backend = backend.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if event.id == quit_id {
                    dispatch(&tx, Event::Quit, &menu_backend, &ctx);
                } else if event.id == launcher_id {
                    dispatch(&tx, Event::OpenLauncher, &menu_backend, &ctx);
                } else if event.id == json_id {
                    dispatch(&tx, Event::OpenJson, &menu_backend, &ctx);
                } else if event.id == timestamp_id {
                    dispatch(&tx, Event::OpenTimestamp, &menu_backend, &ctx);
                } else if event.id == settings_id {
                    dispatch(&tx, Event::OpenSettings, &menu_backend, &ctx);
                } else if event.id == open_id || event.id == queue_id {
                    dispatch(
                        &tx,
                        Event::Open(event.id == queue_id, 0, None),
                        &menu_backend,
                        &ctx,
                    );
                }
            }));
            let icon_image =
                image::load_from_memory(include_bytes!("../icons/clibo-tray.png"))?.into_rgba8();
            let (w, h) = icon_image.dimensions();
            let icon = tray_icon::Icon::from_rgba(icon_image.into_raw(), w, h)?;
            let tray = match TrayIconBuilder::new()
                .with_tooltip("Clibo")
                .with_icon(icon)
                .with_menu(Box::new(menu))
                .build()
            {
                Ok(tray) => Some(tray),
                Err(e) => {
                    backend.report(Err(format!("托盘不可用，关闭按钮将退出：{e}")));
                    None
                }
            };
            if background && tray.is_some() {
                backend.visible.store(false, Ordering::Relaxed);
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            backend.monitor();
            Ok(Box::new(App {
                backend,
                _instance: instance,
                _tray: tray,
                hotkeys,
                registered,
                hotkey_error,
                hotkey_capture: None,
                capture_error: String::new(),
                events: rx,
                excluded: settings.excluded_apps.join("\n"),
                launcher_roots: settings.launcher_roots.join("\n"),
                settings,
                query: String::new(),
                command_selected: 0,
                settings_query: String::new(),
                json: json_ui::JsonPage::initial(),
                json_window: json_ui::WindowTransition::default(),
                timestamp: timestamp_ui::TimestampPage::default(),
                launcher: launcher_ui::LauncherPage::initial(),
                calculator: {
                    let mut calc = crate::calculator::Calculator::default();
                    if std::env::var_os("CLIBO_DATA_DIR").is_some()
                        && std::env::args().any(|a| a == "--demo")
                        && std::env::args().any(|a| a == "--calculator")
                    {
                        for expression in ["1200/8", "99*0.85"] {
                            calc.enter(expression);
                            calc.confirm(crate::calculator::evaluate(expression).unwrap());
                        }
                        calc.enter(
                            &std::env::var("CLIBO_CALCULATOR_EXPRESSION")
                                .unwrap_or_else(|_| "128*3+56".into()),
                        );
                    }
                    calc
                },
                filter: "all".into(),
                group: String::new(),
                items: vec![],
                selected: String::new(),
                revision: 0,
                dirty: true,
                focus_search: true,
                scroll_selected: false,
                history_offset: 0.,
                history_up_distance: 0.,
                history_top: false,
                target,
                owner: 0,
                pinned: std::env::args().any(|a| a == "--pinned" || a == "--smoke"),
                was_focused: false,
                #[cfg(windows)]
                autostart: crate::autostart::enabled(),
                settings_open: std::env::args().any(|a| a == "--smoke")
                    && std::env::args().any(|a| a == "--settings"),
                settings_section: SettingsSection::ALL
                    .into_iter()
                    .find(|s| s.label() == std::env::var("CLIBO_SMOKE_SECTION").unwrap_or_default())
                    .unwrap_or(SettingsSection::Clipboard),
                group_name: String::new(),
                note: String::new(),
                memberships: vec![],
                preview_id: String::new(),
                preview_text: String::new(),
                texture: None,
                enlarged_image: None,
                image_actual_size: false,
                transform: "original".into(),
                confirm: None,
                preview_open: ui_state.preview_open,
                preview_default: ui_state.preview_open,
                ime_composing: false,
                smoke: std::env::args()
                    .any(|a| a == "--smoke")
                    .then(std::time::Instant::now),
                screenshot_requested: false,
                dark: ui_state.dark,
                theme: ui_state.theme,
                thumbs: HashMap::new(),
                thumb_order: VecDeque::new(),
                thumb_budget: THUMB_BUDGET,
                preview_is_image: false,
                image_tx,
                image_rx,
                image_pending: HashSet::new(),
                image_failed: HashSet::new(),
                image_epoch: 0,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

#[derive(Clone, Copy)]
struct Palette {
    panel: Color32,
    card: Color32,
    card_hover: Color32,
    card_selected: Color32,
    border: Color32,
    hairline: Color32,
    text: Color32,
    text_dim: Color32,
    accent: Color32,
    accent_soft: Color32,
    on_accent: Color32,
    star: Color32,
    error: Color32,
    input: Color32,
}
/// 双主题：琥珀 = "精密仪器"（暖黑曜石 + 琥珀信号色，浅色为暖纸面 + 墨色）；
/// 蓝灰 = 启动器原配色（深空蓝黑 + 长春花蓝信号色，浅色为冷纸面）。
fn palette(dark: bool, theme: Theme) -> Palette {
    if theme == Theme::Blue {
        if dark {
            return Palette {
                panel: Color32::from_rgb(0x1B, 0x1D, 0x23),
                card: Color32::from_rgb(0x22, 0x25, 0x2D),
                card_hover: Color32::from_rgb(0x2A, 0x2E, 0x39),
                card_selected: Color32::from_rgb(0x2F, 0x35, 0x50),
                border: Color32::from_rgb(0x3A, 0x3F, 0x4C),
                hairline: Color32::from_rgb(0x2C, 0x2F, 0x3A),
                text: Color32::from_rgb(0xE7, 0xEA, 0xF2),
                text_dim: Color32::from_rgb(0x8F, 0x96, 0xA6),
                accent: Color32::from_rgb(0x9E, 0xAA, 0xFF),
                accent_soft: Color32::from_rgb(0x2A, 0x2F, 0x4E),
                on_accent: Color32::from_rgb(0x17, 0x1A, 0x2E),
                star: Color32::from_rgb(0xB8, 0xC3, 0xFF),
                error: Color32::from_rgb(0xE0, 0x64, 0x5A),
                input: Color32::from_rgb(0x17, 0x19, 0x1F),
            };
        }
        return Palette {
            panel: Color32::from_rgb(0xED, 0xF0, 0xF5),
            card: Color32::from_rgb(0xF8, 0xFA, 0xFD),
            card_hover: Color32::from_rgb(0xE4, 0xE9, 0xF2),
            card_selected: Color32::from_rgb(0xDB, 0xE3, 0xFA),
            border: Color32::from_rgb(0xD1, 0xD7, 0xE2),
            hairline: Color32::from_rgb(0xDE, 0xE3, 0xEC),
            text: Color32::from_rgb(0x1B, 0x1E, 0x27),
            text_dim: Color32::from_rgb(0x6B, 0x72, 0x80),
            accent: Color32::from_rgb(0x4C, 0x5F, 0xD7),
            accent_soft: Color32::from_rgb(0xE5, 0xE9, 0xFB),
            on_accent: Color32::WHITE,
            star: Color32::from_rgb(0x6B, 0x7B, 0xE8),
            error: Color32::from_rgb(0xB3, 0x26, 0x1E),
            input: Color32::from_rgb(0xFB, 0xFC, 0xFE),
        };
    }
    if dark {
        Palette {
            panel: Color32::from_rgb(0x12, 0x11, 0x10),
            card: Color32::from_rgb(0x1B, 0x19, 0x17),
            card_hover: Color32::from_rgb(0x25, 0x22, 0x1F),
            card_selected: Color32::from_rgb(0x2E, 0x25, 0x17),
            border: Color32::from_rgb(0x35, 0x30, 0x2A),
            hairline: Color32::from_rgb(0x27, 0x24, 0x21),
            text: Color32::from_rgb(0xEA, 0xE5, 0xDA),
            text_dim: Color32::from_rgb(0x90, 0x89, 0x7C),
            accent: Color32::from_rgb(0xED, 0xA3, 0x3A),
            accent_soft: Color32::from_rgb(0x2B, 0x23, 0x15),
            on_accent: Color32::from_rgb(0x22, 0x18, 0x06),
            star: Color32::from_rgb(0xF0, 0xB9, 0x5A),
            error: Color32::from_rgb(0xE0, 0x64, 0x5A),
            input: Color32::from_rgb(0x0E, 0x0D, 0x0C),
        }
    } else {
        Palette {
            panel: Color32::from_rgb(0xF1, 0xED, 0xE4),
            card: Color32::from_rgb(0xFB, 0xF9, 0xF3),
            card_hover: Color32::from_rgb(0xEF, 0xE9, 0xDC),
            card_selected: Color32::from_rgb(0xF5, 0xE4, 0xC3),
            border: Color32::from_rgb(0xDC, 0xD4, 0xC3),
            hairline: Color32::from_rgb(0xE4, 0xDE, 0xD0),
            text: Color32::from_rgb(0x21, 0x1D, 0x16),
            text_dim: Color32::from_rgb(0x7E, 0x76, 0x67),
            accent: Color32::from_rgb(0xA9, 0x6D, 0x00),
            accent_soft: Color32::from_rgb(0xF3, 0xE7, 0xCB),
            on_accent: Color32::WHITE,
            star: Color32::from_rgb(0xC7, 0x8F, 0x12),
            error: Color32::from_rgb(0xB3, 0x26, 0x1E),
            input: Color32::from_rgb(0xFD, 0xFB, 0xF6),
        }
    }
}
/// UI-only preference; deliberately separate from the shared encrypted Settings.
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Theme {
    #[default]
    Amber,
    Blue,
}
impl Theme {
    fn label(self) -> &'static str {
        match self {
            Theme::Amber => "琥珀",
            Theme::Blue => "蓝灰",
        }
    }
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct UiState {
    #[serde(default)]
    dark: bool,
    #[serde(default)]
    theme: Theme,
    // 历史默认值是打开；serde(default) 给 false，需要 default_preview_open 兜底。
    #[serde(default = "default_preview_open")]
    preview_open: bool,
}
fn default_preview_open() -> bool {
    true
}
impl UiState {
    fn of(app: &App) -> Self {
        Self {
            dark: app.dark,
            theme: app.theme,
            preview_open: app.preview_default,
        }
    }
    fn load(dir: &Path) -> Self {
        std::fs::read(dir.join("ui-state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn save(&self, dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir.join("ui-state.json"), bytes);
        }
    }
}
fn apply_theme(ctx: &egui::Context, dark: bool, theme: Theme) {
    let p = palette(dark, theme);
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.card;
    visuals.extreme_bg_color = p.input;
    visuals.faint_bg_color = p.card_hover;
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.accent;
    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = egui::Stroke::new(1., p.accent);
    let round = CornerRadius::same(4);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = round;
        widget.fg_stroke.color = p.text;
        widget.bg_stroke.width = 1.;
    }
    visuals.widgets.inactive.bg_fill = p.card;
    visuals.widgets.inactive.bg_stroke.color = p.border;
    visuals.widgets.hovered.bg_fill = p.card_hover;
    visuals.widgets.hovered.bg_stroke.color = p.accent;
    visuals.widgets.hovered.fg_stroke.color = p.accent;
    visuals.widgets.active.bg_fill = p.card_selected;
    visuals.widgets.active.bg_stroke.color = p.accent;
    visuals.widgets.open.bg_fill = p.card_hover;
    visuals.widgets.open.bg_stroke.color = p.border;
    visuals.widgets.inactive.weak_bg_fill = p.card;
    visuals.widgets.hovered.weak_bg_fill = p.card_hover;
    visuals.widgets.active.weak_bg_fill = p.card_selected;
    visuals.widgets.open.weak_bg_fill = p.card_hover;
    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke.color = p.hairline;
    visuals.widgets.noninteractive.fg_stroke.color = p.text_dim;
    visuals.window_corner_radius = CornerRadius::same(APP_CORNER_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(6);
    let shadow_alpha = if dark { 90 } else { 40 };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 3],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    // 圆角窗口不需要投影：设置等 egui 窗口与外层页面边缘保持一致的圆角。
    visuals.window_shadow = egui::Shadow::NONE;
    // 页面背景由无框主窗口统一绘制圆角底色，各面板保持透明。
    visuals.panel_fill = Color32::TRANSPARENT;
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(9., 7.);
    style.spacing.button_padding = Vec2::new(10., 5.);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(17., egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(13., egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(11., egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(12., egui::FontFamily::Monospace),
        ),
    ]
    .into();
    style.visuals = visuals;
    ctx.set_style(style);
}
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    #[cfg(windows)]
    let paths = ["C:/Windows/Fonts/simhei.ttf", "C:/Windows/Fonts/msyh.ttc"];
    #[cfg(target_os = "macos")]
    let paths = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    ];
    #[cfg(windows)]
    let mono_paths = [
        "C:/Windows/Fonts/CascadiaMono.ttf",
        "C:/Windows/Fonts/consola.ttf",
    ];
    #[cfg(target_os = "macos")]
    let mono_paths = [
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Monaco.ttf",
    ];
    // egui clones owned font bytes into FontVec. A process-lifetime borrowed
    // buffer lets its FontRef share the same bytes, including across DPI changes.
    static CJK: std::sync::OnceLock<Option<Box<[u8]>>> = std::sync::OnceLock::new();
    static MONO: std::sync::OnceLock<Option<Box<[u8]>>> = std::sync::OnceLock::new();
    // Latin metadata renders in a real monospace face; CJK stays as fallback.
    if let Some(data) = MONO.get_or_init(|| {
        mono_paths
            .iter()
            .find_map(|path| std::fs::read(path).ok().map(Vec::into_boxed_slice))
    }) {
        fonts
            .font_data
            .insert("mono-ui".into(), egui::FontData::from_static(data).into());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "mono-ui".into());
    }
    if let Some(data) = CJK.get_or_init(|| {
        paths
            .iter()
            .find_map(|path| std::fs::read(path).ok().map(Vec::into_boxed_slice))
    }) {
        fonts
            .font_data
            .insert("cjk".into(), egui::FontData::from_static(data).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}
/// Monospace helper for metadata, counters and keyboard hints.
fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).family(egui::FontFamily::Monospace)
}
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}
/// Small caps section label with a trailing hairline rule.
fn section(ui: &mut egui::Ui, p: &Palette, label: &str) {
    ui.horizontal(|ui| {
        ui.label(mono(label).size(10.5).color(p.text_dim));
        let rect = ui.available_rect_before_wrap();
        ui.painter().hline(
            egui::Rangef::new(rect.left() + 4., rect.right()),
            rect.center().y,
            egui::Stroke::new(1., p.hairline),
        );
    });
}
/// Pill toggle used for filter chips; active state carries the accent.
fn pill(ui: &mut egui::Ui, p: &Palette, label: &str, active: bool) -> egui::Response {
    // Exact geometry avoids font metrics and nested layouts changing chip height.
    let galley = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::monospace(11.),
        if active { p.accent } else { p.text_dim },
    );
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 20., 24.), egui::Sense::click());
    let hover = response.hovered();
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        if active {
            p.accent_soft
        } else if hover {
            p.card_hover
        } else {
            Color32::TRANSPARENT
        },
        egui::Stroke::new(1., if active || hover { p.accent } else { p.border }),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2., galley, p.text_dim);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            active,
            label,
        )
    });
    response
}
/// Frameless text button; hover feedback comes from the accent fg stroke.
fn ghost(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).size(12.)).frame(false))
}
/// Font-independent line icons with the same geometry and feedback as filter pills.
fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    active: bool,
    hint: &str,
) -> egui::Response {
    let response = ui.add(
        egui::Button::new("")
            .min_size(Vec2::splat(26.))
            .corner_radius(CornerRadius::same(13))
            .fill(if active {
                p.accent_soft
            } else {
                Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::new(
                1.,
                if active { p.accent } else { p.border },
            )),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), hint));
    let c = response.rect.center();
    let painter = ui.painter();
    let stroke = egui::Stroke::new(
        1.4,
        if active || response.hovered() {
            p.accent
        } else {
            p.text_dim
        },
    );
    let line = |a: [f32; 2], b: [f32; 2]| {
        painter.line_segment([c + Vec2::from(a), c + Vec2::from(b)], stroke);
    };
    match icon {
        "pin" => {
            line([-3., -6.], [3., -6.]);
            line([-2., -6.], [-2., -1.]);
            line([2., -6.], [2., -1.]);
            line([-2., -1.], [-5., 2.]);
            line([2., -1.], [5., 2.]);
            line([-5., 2.], [5., 2.]);
            line([0., 2.], [0., 7.]);
        }
        "sun" | "settings" => {
            painter.circle_stroke(c, if icon == "sun" { 3. } else { 4.5 }, stroke);
            if icon == "settings" {
                painter.circle_stroke(c, 1.5, stroke);
            }
            for i in 0..8 {
                let v = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.);
                painter.line_segment([c + v * 5.5, c + v * 7.], stroke);
            }
        }
        "moon" => {
            painter.add(egui::Shape::line(
                vec![
                    c + Vec2::new(2., -6.),
                    c + Vec2::new(-2., -5.),
                    c + Vec2::new(-5., -1.),
                    c + Vec2::new(-4., 4.),
                    c + Vec2::new(0., 6.),
                    c + Vec2::new(5., 4.),
                    c + Vec2::new(1., 3.),
                    c + Vec2::new(-1., 0.),
                    c + Vec2::new(0., -4.),
                    c + Vec2::new(2., -6.),
                ],
                stroke,
            ));
        }
        "close" => {
            line([-4., -4.], [4., 4.]);
            line([4., -4.], [-4., 4.]);
        }
        _ => {
            line([-5., -2.], [0., 3.]);
            line([0., 3.], [5., -2.]);
            line([-5., 6.], [5., 6.]);
        }
    }
    response.on_hover_text(hint)
}
/// Accent-filled primary action.
fn primary(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.).strong().color(p.on_accent))
            .fill(p.accent)
            .stroke(egui::Stroke::NONE)
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(0., 22.)),
    )
}
/// Danger action using the same pill geometry as the rest of the UI.
fn danger_pill(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(mono(label).size(11.).color(p.error))
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1., p.error))
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(0., 22.)),
    )
}
/// Outlined mono tag for the entry kind.
fn kind_tag(ui: &mut egui::Ui, p: &Palette, label: &str) {
    egui::Frame::default()
        .stroke(egui::Stroke::new(1., p.border))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(mono(label).size(10.).color(p.accent));
        });
}
/// Borderless icon button for the search row; only hover reveals the chip.
fn ghost_icon(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    active: bool,
    hint: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), hint));
    let hovered = response.hovered();
    if hovered || active {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(7),
            if active { p.accent_soft } else { p.card_hover },
        );
    }
    let c = rect.center();
    let stroke = egui::Stroke::new(
        1.4,
        if active || hovered {
            p.accent
        } else {
            p.text_dim
        },
    );
    let painter = ui.painter();
    let line = |a: [f32; 2], b: [f32; 2]| {
        painter.line_segment([c + Vec2::from(a), c + Vec2::from(b)], stroke);
    };
    match icon {
        "pin" => {
            line([-3., -6.], [3., -6.]);
            line([-2., -6.], [-2., -1.]);
            line([2., -6.], [2., -1.]);
            line([-2., -1.], [-5., 2.]);
            line([2., -1.], [5., 2.]);
            line([-5., 2.], [5., 2.]);
            line([0., 2.], [0., 7.]);
        }
        "sun" | "settings" => {
            painter.circle_stroke(c, if icon == "sun" { 3. } else { 4.5 }, stroke);
            if icon == "settings" {
                painter.circle_stroke(c, 1.5, stroke);
            }
            for i in 0..8 {
                let v = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.);
                painter.line_segment([c + v * 5.5, c + v * 7.], stroke);
            }
        }
        "moon" => {
            painter.add(egui::Shape::line(
                vec![
                    c + Vec2::new(2., -6.),
                    c + Vec2::new(-2., -5.),
                    c + Vec2::new(-5., -1.),
                    c + Vec2::new(-4., 4.),
                    c + Vec2::new(0., 6.),
                    c + Vec2::new(5., 4.),
                    c + Vec2::new(1., 3.),
                    c + Vec2::new(-1., 0.),
                    c + Vec2::new(0., -4.),
                    c + Vec2::new(2., -6.),
                ],
                stroke,
            ));
        }
        _ => {}
    }
    response.on_hover_text(hint)
}
/// Keyboard-hint chip used in the footer (`kbd` look).
fn kbd(ui: &mut egui::Ui, p: &Palette, label: &str) {
    egui::Frame::default()
        .fill(p.card)
        .stroke(egui::Stroke::new(1., p.border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(mono(label).size(9.5).color(p.text_dim));
        });
}
/// Amber toggle switch for settings rows; returns true when the value changed.
fn toggle(ui: &mut egui::Ui, p: &Palette, value: &mut bool, hint: &str) -> bool {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(36., 20.), egui::Sense::click());
    let mut changed = false;
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
        changed = true;
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *value, 0.15);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), mix(p.border, p.accent, t));
    let r = 7.;
    let cx = rect.left() + 3. + r + t * (rect.width() - 6. - 2. * r);
    ui.painter().circle_filled(
        egui::Pos2::new(cx, rect.center().y),
        r,
        mix(p.text_dim, p.on_accent, t),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), hint));
    if !hint.is_empty() {
        response.on_hover_text(hint);
    }
    changed
}
/// − value + stepper used for numeric settings.
fn stepper<T>(ui: &mut egui::Ui, p: &Palette, value: &mut T, min: T, max: T, step: T)
where
    T: Copy
        + PartialOrd
        + std::ops::Add<Output = T>
        + std::ops::Sub<Output = T>
        + std::fmt::Display,
{
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.;
        for (label, sign) in [("−", -1i8), ("+", 1)] {
            let button = ui.add(
                egui::Button::new(mono(label).size(12.).color(p.text_dim))
                    .fill(p.input)
                    .stroke(egui::Stroke::new(1., p.border))
                    .corner_radius(CornerRadius::same(6))
                    .min_size(Vec2::splat(22.)),
            );
            if button.clicked() {
                let next = if sign < 0 {
                    *value - step
                } else {
                    *value + step
                };
                if next < min {
                    *value = min;
                } else if next > max {
                    *value = max;
                } else {
                    *value = next;
                }
            }
            if sign < 0 {
                ui.add_sized(
                    Vec2::new(56., 22.),
                    egui::Label::new(mono(format!("{value}")).size(12.).color(p.text))
                        .halign(egui::Align::Center),
                );
            }
        }
    });
}
/// Grouped card with a small caps header, matching the settings redesign.
fn settings_card(ui: &mut egui::Ui, p: &Palette, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(p.card)
        .stroke(egui::Stroke::new(1., p.hairline))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::default()
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(mono(title).size(11.).strong().color(p.text_dim));
                });
            ui.painter().hline(
                ui.available_rect_before_wrap().x_range(),
                ui.cursor().top(),
                egui::Stroke::new(1., p.hairline),
            );
            add(ui);
        });
    ui.add_space(14.);
}
/// Hairline between rows inside a settings card.
fn card_divider(ui: &mut egui::Ui, p: &Palette) {
    ui.painter().hline(
        ui.available_rect_before_wrap().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1., p.hairline),
    );
}
/// Settings row: label + description on the left, control on the right.
fn setting_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    desc: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    setting_row_sized(ui, p, title, desc, 170., control)
}
/// Same row with a custom reserved control width for wider controls (e.g. hotkey chips).
fn setting_row_sized(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    desc: &str,
    control_width: f32,
    control: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::default()
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - control_width).max(200.));
                    ui.label(RichText::new(title).size(12.8).color(p.text));
                    if !desc.is_empty() {
                        ui.label(RichText::new(desc).size(10.8).color(p.text_dim));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    control(ui);
                });
            });
        });
}
/// Settings left-nav row: self-drawn icon + label, accent highlight when active.
fn settings_nav_item(
    ui: &mut egui::Ui,
    p: &Palette,
    section: SettingsSection,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), egui::Sense::click());
    if active {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), p.accent_soft);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), p.card_hover);
    }
    let color = if active { p.accent } else { p.text_dim };
    let icon = egui::Rect::from_center_size(
        egui::Pos2::new(rect.left() + 16., rect.center().y),
        Vec2::splat(14.),
    );
    let stroke = egui::Stroke::new(1.3, color);
    let c = icon.center();
    match section {
        SettingsSection::General => {
            ui.painter().circle_stroke(c, 3.6, stroke);
            for i in 0..4 {
                let a = std::f32::consts::FRAC_PI_4 + i as f32 * std::f32::consts::FRAC_PI_2;
                let dir = egui::Vec2::new(a.cos(), a.sin());
                ui.painter()
                    .line_segment([c + dir * 5., c + dir * 6.8], stroke);
            }
        }
        SettingsSection::Clipboard => {
            ui.painter().rect_stroke(
                icon.shrink(2.),
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.left() + 4., c.y - 1.5),
                    egui::Pos2::new(icon.right() - 4., c.y - 1.5),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.left() + 4., c.y + 2.),
                    egui::Pos2::new(icon.right() - 4., c.y + 2.),
                ],
                stroke,
            );
        }
        SettingsSection::Launcher => {
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.left() + 3.5, icon.top() + 2.),
                    egui::Pos2::new(icon.right() - 2.5, c.y),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.right() - 2.5, c.y),
                    egui::Pos2::new(icon.left() + 3.5, icon.bottom() - 2.),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.left() + 3.5, icon.top() + 2.),
                    egui::Pos2::new(icon.left() + 3.5, icon.bottom() - 2.),
                ],
                stroke,
            );
        }
        SettingsSection::Hotkeys => {
            ui.painter().rect_stroke(
                icon.shrink(1.5),
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Inside,
            );
            for i in -1..=1 {
                ui.painter()
                    .circle_filled(c + egui::Vec2::new(i as f32 * 3.6, 0.), 0.9, color);
            }
        }
        SettingsSection::Data => {
            let top = egui::Rect::from_center_size(
                egui::Pos2::new(c.x, icon.top() + 3.),
                Vec2::new(10., 4.6),
            );
            ui.painter().line_segment(
                [
                    top.left_bottom(),
                    top.left_bottom() + egui::Vec2::new(0., 7.6),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    top.right_bottom(),
                    top.right_bottom() + egui::Vec2::new(0., 7.6),
                ],
                stroke,
            );
            ui.painter()
                .circle_stroke(egui::Pos2::new(c.x, icon.top() + 3.), 5., stroke);
            ui.painter().line_segment(
                [
                    egui::Pos2::new(icon.left() + 2., icon.bottom() - 2.4),
                    egui::Pos2::new(icon.right() - 2., icon.bottom() - 2.4),
                ],
                stroke,
            );
        }
    }
    ui.painter().text(
        egui::Pos2::new(rect.left() + 30., rect.center().y),
        egui::Align2::LEFT_CENTER,
        section.label(),
        egui::FontId::proportional(12.5),
        if active { p.accent } else { p.text },
    );
    response
}
/// egui logical key → physical code accepted by the hotkey registrar.
fn key_code(key: egui::Key) -> Option<Code> {
    use egui::Key as K;
    let code = match key {
        K::A => Code::KeyA,
        K::B => Code::KeyB,
        K::C => Code::KeyC,
        K::D => Code::KeyD,
        K::E => Code::KeyE,
        K::F => Code::KeyF,
        K::G => Code::KeyG,
        K::H => Code::KeyH,
        K::I => Code::KeyI,
        K::J => Code::KeyJ,
        K::K => Code::KeyK,
        K::L => Code::KeyL,
        K::M => Code::KeyM,
        K::N => Code::KeyN,
        K::O => Code::KeyO,
        K::P => Code::KeyP,
        K::Q => Code::KeyQ,
        K::R => Code::KeyR,
        K::S => Code::KeyS,
        K::T => Code::KeyT,
        K::U => Code::KeyU,
        K::V => Code::KeyV,
        K::W => Code::KeyW,
        K::X => Code::KeyX,
        K::Y => Code::KeyY,
        K::Z => Code::KeyZ,
        K::Num0 => Code::Digit0,
        K::Num1 => Code::Digit1,
        K::Num2 => Code::Digit2,
        K::Num3 => Code::Digit3,
        K::Num4 => Code::Digit4,
        K::Num5 => Code::Digit5,
        K::Num6 => Code::Digit6,
        K::Num7 => Code::Digit7,
        K::Num8 => Code::Digit8,
        K::Num9 => Code::Digit9,
        K::F1 => Code::F1,
        K::F2 => Code::F2,
        K::F3 => Code::F3,
        K::F4 => Code::F4,
        K::F5 => Code::F5,
        K::F6 => Code::F6,
        K::F7 => Code::F7,
        K::F8 => Code::F8,
        K::F9 => Code::F9,
        K::F10 => Code::F10,
        K::F11 => Code::F11,
        K::F12 => Code::F12,
        K::F13 => Code::F13,
        K::F14 => Code::F14,
        K::F15 => Code::F15,
        K::F16 => Code::F16,
        K::F17 => Code::F17,
        K::F18 => Code::F18,
        K::F19 => Code::F19,
        K::F20 => Code::F20,
        K::F21 => Code::F21,
        K::F22 => Code::F22,
        K::F23 => Code::F23,
        K::F24 => Code::F24,
        K::F25 => Code::F25,
        K::F26 => Code::F26,
        K::F27 => Code::F27,
        K::F28 => Code::F28,
        K::F29 => Code::F29,
        K::F30 => Code::F30,
        K::F31 => Code::F31,
        K::F32 => Code::F32,
        K::F33 => Code::F33,
        K::F34 => Code::F34,
        K::F35 => Code::F35,
        K::ArrowUp => Code::ArrowUp,
        K::ArrowDown => Code::ArrowDown,
        K::ArrowLeft => Code::ArrowLeft,
        K::ArrowRight => Code::ArrowRight,
        K::Escape => Code::Escape,
        K::Tab => Code::Tab,
        K::Backspace => Code::Backspace,
        K::Enter => Code::Enter,
        K::Space => Code::Space,
        K::Insert => Code::Insert,
        K::Delete => Code::Delete,
        K::Home => Code::Home,
        K::End => Code::End,
        K::PageUp => Code::PageUp,
        K::PageDown => Code::PageDown,
        K::Comma => Code::Comma,
        K::Period => Code::Period,
        K::Slash => Code::Slash,
        K::Backslash => Code::Backslash,
        K::Minus => Code::Minus,
        K::Equals => Code::Equal,
        K::Semicolon => Code::Semicolon,
        K::Quote => Code::Quote,
        K::Backtick => Code::Backquote,
        K::OpenBracket => Code::BracketLeft,
        K::CloseBracket => Code::BracketRight,
        _ => return None,
    };
    Some(code)
}
/// egui modifier state → global-hotkey modifiers; Cmd/Meta normalizes to SUPER.
fn hotkey_modifiers(m: egui::Modifiers) -> HotkeyMods {
    let mut out = HotkeyMods::empty();
    if m.ctrl {
        out |= HotkeyMods::CONTROL;
    }
    if m.alt {
        out |= HotkeyMods::ALT;
    }
    if m.shift {
        out |= HotkeyMods::SHIFT;
    }
    if m.mac_cmd {
        out |= HotkeyMods::META;
    }
    out
}

fn consume_local_shortcut(ctx: &egui::Context, raw: &str) -> bool {
    let Ok(shortcut) = HotKey::from_str(raw) else {
        return false;
    };
    ctx.input_mut(|input| {
        let mut matched = false;
        let current_mods = hotkey_modifiers(input.modifiers);
        input.events.retain(|event| {
            let pressed = match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => key_code(*key).map(|code| (code, hotkey_modifiers(*modifiers))),
                egui::Event::Copy => Some((Code::KeyC, current_mods)),
                egui::Event::Cut => Some((Code::KeyX, current_mods)),
                egui::Event::Paste(_) => Some((Code::KeyV, current_mods)),
                _ => None,
            };
            let hit = pressed.is_some_and(|(code, mods)| HotKey::new(Some(mods), code) == shortcut);
            matched |= hit;
            !hit
        });
        matched
    })
}

impl App {
    /// 面板激活时生效的页面快捷键（JSON / 时间戳），返回 true 表示本帧已消费按键。
    fn consume_page_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        if self.hotkey_capture.is_some() || self.ime_composing {
            return false;
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if !focused {
            return false;
        }
        let settings = self.backend.store.lock().unwrap().settings.clone();
        for (raw, json) in [
            (&settings.json_hotkey, true),
            (&settings.timestamp_hotkey, false),
        ] {
            if !raw.is_empty() && consume_local_shortcut(ctx, raw) {
                self.calculator.leave();
                self.launcher.active = false;
                self.json.active = json;
                self.timestamp.active = !json;
                self.settings_open = false;
                self.confirm = None;
                self.enlarged_image = None;
                if json {
                    self.json.find_open = false;
                    self.json.history_open = false;
                }
                self.focus_search = true;
                self.dirty = true;
                return true;
            }
        }
        false
    }
    fn consume_find_shortcut(&self, ctx: &egui::Context) -> bool {
        if self.hotkey_capture.is_some() || self.ime_composing {
            return false;
        }
        let shortcut = self
            .backend
            .store
            .lock()
            .unwrap()
            .settings
            .find_hotkey
            .clone();
        consume_local_shortcut(ctx, &shortcut)
    }
}
/// Windows VK → 可注册按键；字母/数字用字面 VK（0x41-0x5A / 0x30-0x39）。
#[cfg(windows)]
const HOTKEY_VKS: &[(u16, Code)] = &[
    (0x30, Code::Digit0),
    (0x31, Code::Digit1),
    (0x32, Code::Digit2),
    (0x33, Code::Digit3),
    (0x34, Code::Digit4),
    (0x35, Code::Digit5),
    (0x36, Code::Digit6),
    (0x37, Code::Digit7),
    (0x38, Code::Digit8),
    (0x39, Code::Digit9),
    (0x41, Code::KeyA),
    (0x42, Code::KeyB),
    (0x43, Code::KeyC),
    (0x44, Code::KeyD),
    (0x45, Code::KeyE),
    (0x46, Code::KeyF),
    (0x47, Code::KeyG),
    (0x48, Code::KeyH),
    (0x49, Code::KeyI),
    (0x4A, Code::KeyJ),
    (0x4B, Code::KeyK),
    (0x4C, Code::KeyL),
    (0x4D, Code::KeyM),
    (0x4E, Code::KeyN),
    (0x4F, Code::KeyO),
    (0x50, Code::KeyP),
    (0x51, Code::KeyQ),
    (0x52, Code::KeyR),
    (0x53, Code::KeyS),
    (0x54, Code::KeyT),
    (0x55, Code::KeyU),
    (0x56, Code::KeyV),
    (0x57, Code::KeyW),
    (0x58, Code::KeyX),
    (0x59, Code::KeyY),
    (0x5A, Code::KeyZ),
    (VK_NUMPAD0, Code::Numpad0),
    (VK_NUMPAD1, Code::Numpad1),
    (VK_NUMPAD2, Code::Numpad2),
    (VK_NUMPAD3, Code::Numpad3),
    (VK_NUMPAD4, Code::Numpad4),
    (VK_NUMPAD5, Code::Numpad5),
    (VK_NUMPAD6, Code::Numpad6),
    (VK_NUMPAD7, Code::Numpad7),
    (VK_NUMPAD8, Code::Numpad8),
    (VK_NUMPAD9, Code::Numpad9),
    (VK_ESCAPE, Code::Escape),
    (VK_TAB, Code::Tab),
    (VK_RETURN, Code::Enter),
    (VK_BACK, Code::Backspace),
    (VK_SPACE, Code::Space),
    (VK_INSERT, Code::Insert),
    (VK_DELETE, Code::Delete),
    (VK_HOME, Code::Home),
    (VK_END, Code::End),
    (VK_PRIOR, Code::PageUp),
    (VK_NEXT, Code::PageDown),
    (VK_LEFT, Code::ArrowLeft),
    (VK_UP, Code::ArrowUp),
    (VK_RIGHT, Code::ArrowRight),
    (VK_DOWN, Code::ArrowDown),
    (VK_F1, Code::F1),
    (VK_F2, Code::F2),
    (VK_F3, Code::F3),
    (VK_F4, Code::F4),
    (VK_F5, Code::F5),
    (VK_F6, Code::F6),
    (VK_F7, Code::F7),
    (VK_F8, Code::F8),
    (VK_F9, Code::F9),
    (VK_F10, Code::F10),
    (VK_F11, Code::F11),
    (VK_F12, Code::F12),
    (VK_F13, Code::F13),
    (VK_F14, Code::F14),
    (VK_F15, Code::F15),
    (VK_F16, Code::F16),
    (VK_F17, Code::F17),
    (VK_F18, Code::F18),
    (VK_F19, Code::F19),
    (VK_F20, Code::F20),
    (VK_F21, Code::F21),
    (VK_F22, Code::F22),
    (VK_F23, Code::F23),
    (VK_F24, Code::F24),
    (VK_OEM_1, Code::Semicolon),
    (VK_OEM_PLUS, Code::Equal),
    (VK_OEM_COMMA, Code::Comma),
    (VK_OEM_MINUS, Code::Minus),
    (VK_OEM_PERIOD, Code::Period),
    (VK_OEM_2, Code::Slash),
    (VK_OEM_3, Code::Backquote),
    (VK_OEM_4, Code::BracketLeft),
    (VK_OEM_5, Code::Backslash),
    (VK_OEM_6, Code::BracketRight),
    (VK_OEM_7, Code::Quote),
];
/// 读取当前物理按下的主键与修饰键（仅本窗口聚焦时，避免录入其他应用的输入）。
#[cfg(windows)]
fn poll_physical_hotkey(focused: bool) -> Option<(Code, HotkeyMods)> {
    if !focused {
        return None;
    }
    unsafe {
        let down = |vk: u16| GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0;
        let mut mods = HotkeyMods::empty();
        if down(VK_CONTROL) {
            mods |= HotkeyMods::CONTROL;
        }
        if down(VK_MENU) {
            mods |= HotkeyMods::ALT;
        }
        if down(VK_SHIFT) {
            mods |= HotkeyMods::SHIFT;
        }
        if down(VK_LWIN) || down(VK_RWIN) {
            mods |= HotkeyMods::SUPER;
        }
        HOTKEY_VKS
            .iter()
            .find(|(vk, _)| down(*vk))
            .map(|&(_, code)| (code, mods))
    }
}
/// macOS 无法无权限轮询原始键态，仅依赖 egui 事件与剪贴板事件还原。
#[cfg(target_os = "macos")]
fn poll_physical_hotkey(_focused: bool) -> Option<(Code, HotkeyMods)> {
    None
}
/// Canonical stored string ("control+alt+KeyV") → readable label ("Ctrl + Alt + V").
fn pretty_hotkey(raw: &str) -> String {
    raw.split('+')
        .map(|token| {
            let token = token.trim().to_lowercase();
            match token.as_str() {
                "control" | "ctrl" => "Ctrl".into(),
                "alt" => "Alt".into(),
                "shift" => "Shift".into(),
                "super" | "meta" | "cmd" | "command" => {
                    if cfg!(target_os = "macos") {
                        "Cmd".to_string()
                    } else {
                        "Win".to_string()
                    }
                }
                "arrowup" => "↑".into(),
                "arrowdown" => "↓".into(),
                "arrowleft" => "←".into(),
                "arrowright" => "→".into(),
                "escape" => "Esc".into(),
                "pageup" => "PageUp".into(),
                "pagedown" => "PageDown".into(),
                "printscreen" => "PrtSc".into(),
                other if other.starts_with("numpad") => format!("小键盘{}", &other[6..]),
                other => {
                    let key = other
                        .strip_prefix("key")
                        .or_else(|| other.strip_prefix("digit"))
                        .unwrap_or(other);
                    let mut chars = key.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => key.to_string(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}
/// Live label of the modifiers currently held while recording.
fn pending_hotkey_label(ctx: &egui::Context) -> String {
    let m = ctx.input(|i| i.modifiers);
    let mut parts: Vec<&str> = Vec::new();
    if m.ctrl {
        parts.push("Ctrl");
    }
    if m.alt {
        parts.push("Alt");
    }
    if m.shift {
        parts.push("Shift");
    }
    if m.mac_cmd {
        parts.push(if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Win"
        });
    }
    if parts.is_empty() {
        "按键…".into()
    } else {
        format!("{} + …", parts.join(" + "))
    }
}
#[cfg(windows)]
fn local_parts(millis: i64) -> (i32, u32, u32, u8, u8) {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    let ticks = (millis + 11_644_473_600_000).saturating_mul(10_000);
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut system = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe {
        if windows_sys::Win32::System::Time::FileTimeToSystemTime(&utc, &mut system) == 0 {
            return (1970, 1, 1, 0, 0);
        }
        let mut local = system;
        windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime(
            std::ptr::null(),
            &system,
            &mut local,
        );
        (
            local.wYear as i32,
            local.wMonth as u32,
            local.wDay as u32,
            local.wHour as u8,
            local.wMinute as u8,
        )
    }
}
#[cfg(target_os = "macos")]
fn local_parts(millis: i64) -> (i32, u32, u32, u8, u8) {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: i64,
        zone: *const i8,
    }
    extern "C" {
        fn localtime_r(clock: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let seconds = millis.div_euclid(1000);
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 1,
        mon: 0,
        year: 70,
        wday: 0,
        yday: 0,
        isdst: 0,
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    unsafe {
        if localtime_r(&seconds, &mut tm).is_null() {
            return (1970, 1, 1, 0, 0);
        }
        (
            tm.year + 1900,
            (tm.mon + 1) as u32,
            tm.mday as u32,
            tm.hour as u8,
            tm.min as u8,
        )
    }
}
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
fn byte_label(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / 1024. / 1024.)
    }
}
fn time_label(copied_at: i64) -> String {
    let now_millis = now();
    let age = now_millis - copied_at;
    if age < 60_000 {
        return "刚刚".into();
    }
    if age < 3_600_000 {
        return format!("{} 分钟前", age / 60_000);
    }
    let (year, month, day, hour, minute) = local_parts(copied_at);
    let (ny, nm, nd, _, _) = local_parts(now_millis);
    match days_from_civil(ny as i64, nm, nd) - days_from_civil(year as i64, month, day) {
        0 => format!("{:02}:{:02}", hour, minute),
        1 => "昨天".into(),
        _ if year == ny => format!("{}月{}日", month, day),
        _ => format!("{year}-{month}-{day}"),
    }
}

impl App {
    /// 预览抽屉的统一写入路径：设置开关与 Ctrl+P 都走这里，改动立即持久化，
    /// 呼出面板时不会回到旧状态。
    fn set_preview(&mut self, open: bool) {
        self.preview_open = open;
        self.preview_default = open;
        if let Ok(dir) = backend::data_dir() {
            UiState::of(self).save(&dir);
        }
    }
    fn hide(&mut self, ctx: &egui::Context) {
        if self._tray.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            return;
        }
        self.backend.visible.store(false, Ordering::Relaxed);
        self.enlarged_image = None;
        self.texture = None;
        self.preview_is_image = false;
        self.preview_id.clear();
        self.preview_text = String::new();
        self.items = Vec::new();
        self.thumbs.clear();
        self.thumb_order.clear();
        self.dirty = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
    fn refresh(&mut self) {
        let revision = self.backend.revision.load(Ordering::Relaxed);
        if !self.dirty && self.revision == revision {
            return;
        }
        let store = self.backend.store.lock().unwrap();
        let query = self.query.strip_prefix("\\=").map(|q| format!("={q}"));
        let ids = backend::matching_ids(
            &store,
            query.as_deref().unwrap_or(&self.query),
            &self.filter,
            &self.group,
        );
        let by_id: std::collections::HashMap<_, _> = store
            .entries
            .iter()
            .map(|e| (&e.view.id, &e.view))
            .collect();
        self.items = ids
            .iter()
            .filter_map(|id| by_id.get(id).map(|v| (*v).clone()))
            .collect();
        if !self.items.iter().any(|e| e.id == self.selected) {
            self.selected = self.items.first().map(|e| e.id.clone()).unwrap_or_default();
        }
        self.revision = revision;
        self.dirty = false;
    }
    fn operation(&mut self, f: impl FnOnce(&mut Store) -> Result<(), String>) {
        let result = f(&mut self.backend.store.lock().unwrap());
        self.backend.report(result);
        self.dirty = true;
    }
    fn apply(&mut self, paste: bool, next: bool, merged: bool) {
        if self.settings_open || self.confirm.is_some() {
            return;
        }
        let transform = if next || merged || self.preview_id != self.selected {
            "original".into()
        } else {
            self.transform.clone()
        };
        self.backend.use_clip(
            self.selected.clone(),
            paste,
            next,
            merged,
            transform,
            self.owner,
            self.target,
        );
    }
    fn queue_image(&mut self, job: ImageJob, ctx: &egui::Context) {
        if job.epoch != self.image_epoch
            || self.image_pending.contains(&job)
            || self.image_failed.contains(&job)
        {
            return;
        }
        match self.image_tx.try_send(job.clone()) {
            Ok(()) => {
                self.image_pending.insert(job);
            }
            Err(mpsc::TrySendError::Full(_)) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.image_failed.insert(job);
                self.backend
                    .report(Err("图片后台处理已停止，请重启 Clibo".into()));
            }
        }
    }
    fn drain_images(&mut self, ctx: &egui::Context) {
        while let Ok(ImageDone { job, result }) = self.image_rx.try_recv() {
            self.image_pending.remove(&job);
            if job.epoch != self.image_epoch {
                continue;
            }
            match result {
                Ok(decoded) => {
                    self.image_failed.remove(&job);
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(decoded.size, &decoded.rgba);
                    match job.kind {
                        ImageKind::Thumbnail => {
                            let id = job.id;
                            let texture =
                                ctx.load_texture(format!("thumb-{id}"), image, Default::default());
                            self.thumbs.insert(id.clone(), texture.clone());
                            if let Some(pos) = self.thumb_order.iter().position(|old| old == &id) {
                                self.thumb_order.remove(pos);
                            }
                            self.thumb_order.push_back(id.clone());
                            while self.thumbs.len() > THUMB_CACHE {
                                if let Some(old) = self.thumb_order.pop_front() {
                                    self.thumbs.remove(&old);
                                } else {
                                    break;
                                }
                            }
                            if self.preview_is_image && self.preview_id == id {
                                self.texture = Some(texture);
                            }
                        }
                        ImageKind::Full(_) => {
                            if self.preview_is_image && self.preview_id == job.id {
                                self.enlarged_image = Some(ctx.load_texture(
                                    "enlarged-image",
                                    image,
                                    Default::default(),
                                ));
                                self.image_actual_size = false;
                            }
                        }
                    }
                }
                Err(error) => {
                    self.image_failed.insert(job);
                    self.backend.report(Err(error));
                }
            }
        }
    }
    fn reset_image_jobs(&mut self) {
        self.image_epoch = self.image_epoch.wrapping_add(1);
        self.image_pending.clear();
        self.image_failed.clear();
        self.thumbs.clear();
        self.thumb_order.clear();
        self.texture = None;
        self.enlarged_image = None;
        self.preview_is_image = false;
        self.preview_id.clear();
    }
    fn backup_history(&mut self) {
        let result = self.backend.store.lock().unwrap().backup_history();
        match result {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "history.manual.db".into());
                *self.backend.status.lock().unwrap() = format!("历史备份完成：{name}");
                self.backend.changed();
            }
            Err(error) => self.backend.report(Err(error)),
        }
    }
    fn restore_history(&mut self) {
        let result = self
            .backend
            .store
            .lock()
            .unwrap()
            .restore_latest_history_backup();
        match result {
            Ok(path) => {
                self.reset_image_jobs();
                self.selected.clear();
                self.dirty = true;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "history.manual.db".into());
                *self.backend.status.lock().unwrap() = format!("已从备份恢复历史：{name}");
                self.backend.changed();
            }
            Err(error) => self.backend.report(Err(error)),
        }
    }
    fn preview(&mut self, ctx: &egui::Context) {
        if self.preview_id == self.selected {
            return;
        }
        self.texture = None;
        self.enlarged_image = None;
        self.preview_text.clear();
        self.preview_is_image = false;
        self.transform = "original".into();
        self.preview_id = self.selected.clone();
        let selected = self.selected.clone();
        let loaded: Result<(String, Vec<String>, bool, Option<String>), String> = {
            let store = self.backend.store.lock().unwrap();
            let Some(entry) = store.entries.iter().find(|e| e.view.id == selected) else {
                return;
            };
            if entry.view.kind == "image" {
                Ok((
                    entry.view.note.clone(),
                    entry.view.group_ids.clone(),
                    true,
                    None,
                ))
            } else {
                store.secret(&selected).map(|secret| {
                    (
                        entry.view.note.clone(),
                        entry.view.group_ids.clone(),
                        false,
                        secret.text,
                    )
                })
            }
        };
        let (note, memberships, is_image, text) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                self.backend.report(Err(error));
                return;
            }
        };
        self.note = note;
        self.memberships = memberships;
        self.preview_is_image = is_image;
        if is_image {
            // 图片行始终自动展开预览抽屉，直接看到大图；下次呼出面板时按偏好复位。
            self.preview_open = true;
            if let Some(texture) = self.thumbs.get(&selected).cloned() {
                self.texture = Some(texture);
            } else {
                self.queue_image(
                    ImageJob {
                        id: selected,
                        kind: ImageKind::Thumbnail,
                        epoch: self.image_epoch,
                    },
                    ctx,
                );
            }
            return;
        }
        if let Some(text) = text {
            self.preview_text = text.chars().take(32_000).collect();
            if text.chars().count() > 32_000 {
                self.preview_text
                    .push_str("\n\n[预览已截断；复制与粘贴使用完整原文]");
            }
        }
    }
    fn enlarge_image(&mut self, ctx: &egui::Context) {
        if !self.preview_is_image || self.selected.is_empty() {
            return;
        }
        let side = ctx.input(|i| i.max_texture_side) as u32;
        self.queue_image(
            ImageJob {
                id: self.selected.clone(),
                kind: ImageKind::Full(side),
                epoch: self.image_epoch,
            },
            ctx,
        );
    }
    /// `>` 命令模式的工具列表，按 `>` 后的文本过滤。
    fn command_items(&self) -> Vec<CommandItem> {
        let settings = self.backend.store.lock().unwrap().settings.clone();
        let pause: CommandItem = if settings.enabled {
            CommandItem {
                id: "pause",
                icon: "‖",
                name: "暂停记录",
                desc: "停止采集剪贴板，已有历史保留",
                meta: "动作".into(),
            }
        } else {
            CommandItem {
                id: "pause",
                icon: "▶",
                name: "恢复记录",
                desc: "继续采集剪贴板新内容",
                meta: "动作".into(),
            }
        };
        let all = vec![
            CommandItem {
                id: "json",
                icon: "{}",
                name: "JSON 格式化",
                desc: "格式化、校验与字段搜索",
                meta: "工具".into(),
            },
            CommandItem {
                id: "timestamp",
                icon: "123",
                name: "时间戳转换",
                desc: "时间戳与日期时间互转",
                meta: "工具".into(),
            },
            CommandItem {
                id: "launcher",
                icon: "▸",
                name: "启动器",
                desc: "搜索应用、文件、书签与网页",
                meta: pretty_hotkey(&settings.launcher_hotkey),
            },
            CommandItem {
                id: "settings",
                icon: "⚙",
                name: "偏好设置",
                desc: "采集、快捷键、备份与分组",
                meta: "工具".into(),
            },
            pause,
        ];
        let filter = self
            .query
            .trim_start_matches(['>', '＞'])
            .trim()
            .to_lowercase();
        if filter.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&filter)
                    || c.desc.to_lowercase().contains(&filter)
                    || c.id.contains(filter.as_str())
            })
            .collect()
    }
    fn run_command(&mut self, id: &str) {
        match id {
            "json" => self.json.active = true,
            "timestamp" => self.timestamp.active = true,
            "launcher" => {
                self.launcher.active = true;
                self.launcher.reopen();
            }
            "settings" => self.open_settings(SettingsSection::General),
            "pause" => self.operation(|s| {
                let mut c = s.settings.clone();
                c.enabled = !c.enabled;
                s.save_settings(c)
            }),
            _ => {}
        }
        self.query.clear();
        self.command_selected = 0;
        self.dirty = true;
        self.focus_search = true;
    }
    /// `>` 命令模式的列表页：与历史行同样的选中卡片语言。
    fn command_page(&mut self, ui: &mut egui::Ui) {
        let p = palette(self.dark, self.theme);
        let commands = self.command_items();
        if commands.is_empty() {
            ui.add_space(80.);
            ui.centered_and_justified(|ui| {
                ui.label(mono("—— 没有匹配的工具 ——").size(13.).color(p.text_dim));
            });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("commands")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(2.);
                for (index, command) in commands.iter().enumerate() {
                    let selected = index == self.command_selected;
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 46.),
                        egui::Sense::click(),
                    );
                    let hover = ui.ctx().animate_bool_with_time(
                        egui::Id::new(("command-hover", command.id)),
                        response.hovered(),
                        0.08,
                    );
                    let fill = if selected {
                        p.card_selected
                    } else {
                        mix(Color32::TRANSPARENT, p.card_hover, hover)
                    };
                    ui.painter()
                        .add(egui::Shape::rect_filled(rect, CornerRadius::same(8), fill));
                    if selected {
                        ui.painter().add(egui::Shape::rect_filled(
                            egui::Rect::from_min_size(
                                rect.min + egui::vec2(1.5, 6.),
                                egui::vec2(2.5, rect.height() - 12.),
                            ),
                            CornerRadius::same(1),
                            p.accent,
                        ));
                    }
                    let mut row = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(egui::Id::new(("command", command.id)))
                            .max_rect(rect.shrink2(Vec2::new(10., 5.))),
                    );
                    row.horizontal(|ui| {
                        let (tic, _) =
                            ui.allocate_exact_size(Vec2::splat(28.), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(tic, CornerRadius::same(7), p.accent_soft);
                        ui.painter().text(
                            tic.center(),
                            egui::Align2::CENTER_CENTER,
                            command.icon,
                            egui::FontId::monospace(11.),
                            p.accent,
                        );
                        ui.add_space(6.);
                        ui.vertical(|ui| {
                            ui.add_space(1.);
                            ui.label(RichText::new(command.name).size(12.8).color(p.text));
                            ui.label(RichText::new(command.desc).size(10.5).color(p.text_dim));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(mono(&command.meta).size(10.5).color(p.text_dim));
                        });
                    });
                    if response.clicked() {
                        let id = command.id;
                        self.run_command(id);
                        return;
                    }
                }
            });
    }
    /// 260px 预览抽屉：头部信息 + 内容 + 备注与分组 + 底部动作。
    fn preview_drawer(&mut self, ui: &mut egui::Ui, height: f32) {
        let p = palette(self.dark, self.theme);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(260., height), egui::Sense::hover());
        ui.painter().vline(
            rect.left(),
            rect.y_range(),
            egui::Stroke::new(1., p.hairline),
        );
        ui.painter().rect_filled(
            rect.shrink2(Vec2::new(0.5, 0.)),
            CornerRadius::same(0),
            p.card,
        );
        let mut drawer = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("preview-drawer")
                .max_rect(rect.shrink2(Vec2::new(1., 0.)))
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        let ui = &mut drawer;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        // 头部：类型 · 大小 / 来源 · 时间
        egui::Frame::default()
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(item) = self
                    .items
                    .iter()
                    .find(|item| item.id == self.selected)
                    .cloned()
                {
                    let kind = match item.kind.as_str() {
                        "image" => "图片",
                        "link" => "链接",
                        _ => "纯文本",
                    };
                    ui.label(
                        RichText::new(format!("{kind} · {}", byte_label(item.bytes)))
                            .size(12.)
                            .strong()
                            .color(p.text),
                    );
                    let time = time_label(item.copied_at);
                    let sub = if item.source.is_empty() {
                        time
                    } else {
                        format!("来自 {} · {time}", item.source)
                    };
                    ui.label(RichText::new(sub).size(10.5).color(p.text_dim));
                } else {
                    ui.label(RichText::new("预览").size(12.).strong().color(p.text));
                    ui.label(
                        RichText::new("选中一条记录查看内容")
                            .size(10.5)
                            .color(p.text_dim),
                    );
                }
            });
        ui.painter().hline(
            ui.available_rect_before_wrap().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1., p.hairline),
        );
        // 内容 + 备注分组（滚动区），底部动作固定。
        let footer_h = 42.;
        egui::ScrollArea::vertical()
            .id_salt("preview-body")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - footer_h).max(80.))
            .show(ui, |ui| {
                egui::Frame::default()
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(texture) = &self.texture {
                            if ui
                                .add(
                                    egui::Image::new(texture)
                                        .max_size(Vec2::new(ui.available_width(), 200.))
                                        .sense(egui::Sense::click())
                                        .corner_radius(CornerRadius::same(6)),
                                )
                                .on_hover_cursor(egui::CursorIcon::ZoomIn)
                                .on_hover_text("点击放大图片")
                                .clicked()
                            {
                                self.enlarge_image(ui.ctx());
                            }
                        } else if self.preview_is_image {
                            egui::Frame::default()
                                .fill(p.input)
                                .stroke(egui::Stroke::new(1., p.hairline))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(8, 18))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new("正在后台生成图片预览…")
                                                .size(11.5)
                                                .color(p.text_dim),
                                        );
                                    });
                                });
                        } else if !self.selected.is_empty() {
                            egui::Frame::default()
                                .fill(p.input)
                                .stroke(egui::Stroke::new(1., p.hairline))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(mono(&self.preview_text).size(12.))
                                            .wrap()
                                            .selectable(true),
                                    );
                                });
                        }
                        if !self.preview_is_image
                            && self.texture.is_none()
                            && !self.selected.is_empty()
                        {
                            ui.add_space(6.);
                            section(ui, &p, "文字处理");
                            ui.add_space(2.);
                            if ghost(ui, "在 JSON 工作页中打开 →").clicked() {
                                let source =
                                    self.backend.store.lock().unwrap().secret(&self.selected);
                                match source {
                                    Ok(secret) => {
                                        self.json.source = secret.text.unwrap_or_default();
                                        self.json.parse();
                                        self.remember_json_history();
                                        self.json.active = true;
                                    }
                                    Err(error) => self.backend.report(Err(error)),
                                }
                            }
                            ui.add_space(2.);
                            let old = self.transform.clone();
                            ui.horizontal_wrapped(|ui| {
                                for (id, label) in [
                                    ("original", "原文"),
                                    ("trim", "去首尾空白"),
                                    ("blank-lines", "合并空行"),
                                    ("upper", "大写"),
                                    ("lower", "小写"),
                                ] {
                                    if pill(ui, &p, label, self.transform == id).clicked() {
                                        self.transform = id.into();
                                    }
                                }
                            });
                            if old != self.transform {
                                let source = self
                                    .backend
                                    .store
                                    .lock()
                                    .unwrap()
                                    .secret(&self.selected)
                                    .ok()
                                    .and_then(|s| s.text)
                                    .unwrap_or_default();
                                match crate::text_tools::transform(&source, &self.transform) {
                                    Ok(text) => {
                                        self.preview_text = text.chars().take(32_000).collect()
                                    }
                                    Err(e) => {
                                        self.transform = old;
                                        self.backend.report(Err(e));
                                    }
                                }
                            }
                        }
                    });
                if !self.selected.is_empty() {
                    ui.painter().hline(
                        ui.available_rect_before_wrap().x_range(),
                        ui.cursor().top(),
                        egui::Stroke::new(1., p.hairline),
                    );
                    egui::Frame::default()
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(mono("备注与分组").size(10.5).strong().color(p.text_dim));
                            ui.add_space(7.);
                            ui.add(
                                egui::TextEdit::multiline(&mut self.note)
                                    .hint_text("备注，不会随内容粘贴")
                                    .desired_rows(2)
                                    .char_limit(1000),
                            );
                            let groups =
                                self.backend.store.lock().unwrap().organizer.groups.clone();
                            if !groups.is_empty() {
                                ui.add_space(8.);
                                ui.horizontal_wrapped(|ui| {
                                    for g in groups {
                                        let selected = self.memberships.contains(&g.id);
                                        if pill(ui, &p, &g.name, selected).clicked() {
                                            self.memberships.retain(|id| id != &g.id);
                                            if !selected {
                                                self.memberships.push(g.id);
                                            }
                                        }
                                    }
                                });
                            }
                            ui.add_space(8.);
                            let width = ui.available_width();
                            if ui
                                .add_sized(
                                    Vec2::new(width, 24.),
                                    egui::Button::new(
                                        RichText::new("保存备注 / 分组")
                                            .size(12.)
                                            .strong()
                                            .color(p.on_accent),
                                    )
                                    .fill(p.accent)
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(CornerRadius::same(6)),
                                )
                                .clicked()
                            {
                                let id = self.selected.clone();
                                let groups = self.memberships.clone();
                                let note = self.note.clone();
                                self.operation(|s| s.annotate(&id, &groups, &note));
                            }
                            ui.add_space(8.);
                            let pinned = self
                                .items
                                .iter()
                                .find(|item| item.id == self.selected)
                                .is_some_and(|item| item.pinned);
                            ui.horizontal(|ui| {
                                if pill(ui, &p, if pinned { "取消收藏" } else { "收藏" }, pinned)
                                    .clicked()
                                {
                                    let id = self.selected.clone();
                                    self.operation(|s| s.pin(&id));
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if danger_pill(ui, &p, "删除记录").clicked() {
                                            self.confirm = Some(self.selected.clone());
                                        }
                                    },
                                );
                            });
                        });
                }
            });
        ui.painter().hline(
            ui.available_rect_before_wrap().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1., p.hairline),
        );
        egui::Frame::default()
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if primary(ui, &p, "粘贴").clicked() {
                        self.apply(true, false, false);
                    }
                    if pill(ui, &p, "仅复制", false).clicked() {
                        self.apply(false, false, false);
                    }
                });
            });
    }
    fn open_settings(&mut self, section: SettingsSection) {
        #[cfg(windows)]
        {
            self.autostart = crate::autostart::enabled();
        }
        self.settings = self.backend.store.lock().unwrap().settings.clone();
        self.excluded = self.settings.excluded_apps.join("\n");
        self.launcher_roots = self.settings.launcher_roots.join("\n");
        self.settings_section = section;
        self.settings_open = true;
        self.hotkey_capture = None;
        self.capture_error.clear();
    }

    fn save_settings(&mut self) {
        let mut next = self.settings.clone();
        next.excluded_apps = self
            .excluded
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let roots: Vec<String> = self
            .launcher_roots
            .lines()
            .map(crate::launcher::search::expand_path)
            .filter(|s| !s.is_empty())
            .collect();
        if roots
            .iter()
            .any(|root| !Path::new(root).is_absolute() || !Path::new(root).is_dir())
        {
            self.backend
                .report(Err("启动器搜索目录必须是存在的绝对路径".into()));
            return;
        }
        next.launcher_roots = roots;
        if let Err(e) = next.validate() {
            self.backend.report(Err(e));
            return;
        }
        let old = self.backend.store.lock().unwrap().settings.clone();
        let launcher_scope_changed = old.launcher_roots != next.launcher_roots
            || old.launcher_bookmarks != next.launcher_bookmarks;
        let launcher_rules_changed = old.launcher_search_prefixes != next.launcher_search_prefixes;
        for key in &self.registered {
            let _ = self.hotkeys.unregister(*key);
        }
        self.registered.clear();
        match register(&self.hotkeys, &next) {
            Ok(keys) => self.registered = keys,
            Err(e) => {
                self.registered = register_available(&self.hotkeys, &old).0;
                self.hotkey_error = e.clone();
                self.backend.report(Err(e));
                return;
            }
        }
        let result = self
            .backend
            .store
            .lock()
            .unwrap()
            .save_settings(next.clone());
        if result.is_err() {
            for key in &self.registered {
                let _ = self.hotkeys.unregister(*key);
            }
            self.registered = register_available(&self.hotkeys, &old).0;
        } else {
            self.settings = next;
            self.hotkey_error.clear();
            self.launcher
                .settings_changed(launcher_scope_changed, launcher_rules_changed);
            self.settings_open = false;
        }
        self.backend.report(result);
    }
    /// While recording is armed, turn the next key press into a hotkey value.
    ///
    /// egui-winit 会把 Ctrl/Cmd+V、C、X 转成剪贴板事件并丢弃按键事件，因此
    /// 先用 egui 事件捕获，再把 Paste/Copy/Cut 还原为 V/C/X；Windows 上还用
    /// GetAsyncKeyState 轮询原始键态兜底（剪贴板为空时 Paste 也不会产生）。
    fn capture_hotkey(&mut self, ctx: &egui::Context) {
        let Some(slot) = self.hotkey_capture else {
            return;
        };
        let mut pressed: Option<(Code, HotkeyMods)> = None;
        let mut unsupported = false;
        let (now_mods, focused) =
            ctx.input(|i| (i.modifiers, i.viewport().focused.unwrap_or(false)));
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if let Some(code) = key_code(*key) {
                            pressed = Some((code, hotkey_modifiers(*modifiers)));
                        } else {
                            unsupported = true;
                        }
                    }
                    // 被 egui-winit 吞掉的 Ctrl/Cmd+V|C|X：从剪贴板事件还原按键。
                    egui::Event::Paste(_) => {
                        pressed = Some((Code::KeyV, hotkey_modifiers(now_mods)))
                    }
                    egui::Event::Copy => pressed = Some((Code::KeyC, hotkey_modifiers(now_mods))),
                    egui::Event::Cut => pressed = Some((Code::KeyX, hotkey_modifiers(now_mods))),
                    _ => {}
                }
            }
        });
        if pressed.is_none() {
            pressed = poll_physical_hotkey(focused);
        }
        let Some((code, mods)) = pressed else {
            if unsupported {
                self.capture_error = "这个按键暂不支持，请换字母、数字、方向键或 F 功能键".into();
            }
            return;
        };
        if code == Code::Escape && mods.is_empty() {
            self.hotkey_capture = None;
            self.capture_error.clear();
            return;
        }
        let needs = if cfg!(target_os = "macos") {
            "组合需包含 Ctrl、Alt 或 Cmd"
        } else {
            "组合需包含 Ctrl 或 Alt"
        };
        if !(mods.contains(HotkeyMods::CONTROL)
            || mods.contains(HotkeyMods::ALT)
            || mods.contains(HotkeyMods::SUPER))
        {
            self.capture_error = needs.into();
            return;
        }
        let hotkey = HotKey::new(Some(mods), code);
        let conflict = [
            (HotkeySlot::Show, &self.settings.hotkey),
            (HotkeySlot::Queue, &self.settings.queue_hotkey),
            (HotkeySlot::Find, &self.settings.find_hotkey),
            (HotkeySlot::Launcher, &self.settings.launcher_hotkey),
            (HotkeySlot::Json, &self.settings.json_hotkey),
            (HotkeySlot::Timestamp, &self.settings.timestamp_hotkey),
        ]
        .iter()
        .any(|(other_slot, other)| {
            *other_slot != slot && HotKey::from_str(other).is_ok_and(|h| h == hotkey)
        });
        if conflict {
            self.capture_error = "与另一个快捷键相同".into();
            return;
        }
        match slot {
            HotkeySlot::Show => self.settings.hotkey = hotkey.to_string(),
            HotkeySlot::Queue => self.settings.queue_hotkey = hotkey.to_string(),
            HotkeySlot::Find => self.settings.find_hotkey = hotkey.to_string(),
            HotkeySlot::Launcher => self.settings.launcher_hotkey = hotkey.to_string(),
            HotkeySlot::Json => self.settings.json_hotkey = hotkey.to_string(),
            HotkeySlot::Timestamp => self.settings.timestamp_hotkey = hotkey.to_string(),
        }
        self.hotkey_capture = None;
        self.capture_error.clear();
    }
    /// 48×36 row thumbnail. The UI only queues bounded work and paints a
    /// placeholder; SQLite reads, decryption, resizing and pixel decoding all run
    /// on the image worker.
    fn row_thumbnail(&mut self, ui: &mut egui::Ui, item: &ClipView) {
        let p = palette(self.dark, self.theme);
        let cached = self.thumbs.get(&item.id).cloned();
        if let Some(texture) = cached {
            if let Some(pos) = self.thumb_order.iter().position(|id| id == &item.id) {
                if let Some(id) = self.thumb_order.remove(pos) {
                    self.thumb_order.push_back(id);
                }
            }
            ui.add(
                egui::Image::from_texture(&texture)
                    .fit_to_exact_size(Vec2::new(48., 36.))
                    .corner_radius(CornerRadius::same(4)),
            );
            return;
        }
        let job = ImageJob {
            id: item.id.clone(),
            kind: ImageKind::Thumbnail,
            epoch: self.image_epoch,
        };
        if !self.image_pending.contains(&job) && !self.image_failed.contains(&job) {
            if self.thumb_budget == 0 {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));
            } else {
                self.thumb_budget -= 1;
                self.queue_image(job, ui.ctx());
            }
        }
        thumb_placeholder(ui, &p);
    }
}
fn thumb_placeholder(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(48., 36.), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), p.card_hover);
}

fn resize_border(ctx: &egui::Context) {
    use egui::viewport::ResizeDirection as D;
    let bounds = ctx.content_rect();
    let Some(pos) = ctx.pointer_hover_pos() else {
        return;
    };
    if !bounds.contains(pos) {
        return;
    }
    let left = pos.x < bounds.left() + 5.;
    let right = pos.x > bounds.right() - 5.;
    let top = pos.y < bounds.top() + 5.;
    let bottom = pos.y > bounds.bottom() - 5.;
    let (direction, cursor) = match (left, right, top, bottom) {
        (true, _, true, _) => (D::NorthWest, egui::CursorIcon::ResizeNwSe),
        (_, true, _, true) => (D::SouthEast, egui::CursorIcon::ResizeNwSe),
        (_, true, true, _) => (D::NorthEast, egui::CursorIcon::ResizeNeSw),
        (true, _, _, true) => (D::SouthWest, egui::CursorIcon::ResizeNeSw),
        (true, _, _, _) => (D::West, egui::CursorIcon::ResizeHorizontal),
        (_, true, _, _) => (D::East, egui::CursorIcon::ResizeHorizontal),
        (_, _, true, _) => (D::North, egui::CursorIcon::ResizeVertical),
        (_, _, _, true) => (D::South, egui::CursorIcon::ResizeVertical),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(started) = self.smoke {
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = screenshot {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                let result = backend::data_dir().and_then(|path| {
                    image::save_buffer(
                        path.join("native-smoke.png"),
                        &bytes,
                        image.width() as u32,
                        image.height() as u32,
                        image::ColorType::Rgba8,
                    )
                    .map_err(|e| e.to_string())
                });
                if let Err(e) = result {
                    eprintln!("{e}");
                }
                self._tray = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            // Allow background catalog and shell icons to settle in launcher smoke captures.
            let capture_delay = if self.launcher.active { 6 } else { 2 };
            if started.elapsed() > std::time::Duration::from_secs(capture_delay)
                && !self.screenshot_requested
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.screenshot_requested = true;
            }
            if started.elapsed() > std::time::Duration::from_secs(15) {
                self._tray = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        #[cfg(windows)]
        if let Ok(handle) = frame.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                self.owner = h.hwnd.get() as usize;
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = frame;
            self.owner = std::process::id() as usize;
        }
        self.backend.window.store(self.owner, Ordering::Relaxed);
        self.drain_images(ctx);
        // 无边框主窗口：在透明背景上绘制圆角底色，让剪贴板、启动器、JSON、
        // 时间戳各页面与设置窗口保持一致的圆角。
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            ctx.content_rect(),
            CornerRadius::same(APP_CORNER_RADIUS),
            palette(self.dark, self.theme).panel,
        );
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::OpenJson
                | Event::OpenTimestamp
                | Event::OpenLauncher
                | Event::LauncherHotkey(_) => {
                    if matches!(event, Event::LauncherHotkey(true)) && self.launcher.active {
                        self.hide(ctx);
                        continue;
                    }
                    self.calculator.leave();
                    self.launcher.active =
                        matches!(event, Event::OpenLauncher | Event::LauncherHotkey(_));
                    if self.launcher.active {
                        self.launcher.reopen();
                    }
                    self.launcher.focus = true;
                    self.json.active = matches!(event, Event::OpenJson);
                    self.timestamp.active = matches!(event, Event::OpenTimestamp);
                    self.settings_open = false;
                    self.hotkey_capture = None;
                    self.confirm = None;
                    self.enlarged_image = None;
                    self.was_focused = false;
                    self.backend.visible.store(true, Ordering::Relaxed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Event::OpenSettings => {
                    self.calculator.leave();
                    self.launcher.active = false;
                    self.json.active = false;
                    self.timestamp.active = false;
                    self.open_settings(SettingsSection::Clipboard);
                    self.confirm = None;
                    self.enlarged_image = None;
                    self.was_focused = false;
                    self.backend.visible.store(true, Ordering::Relaxed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Event::Quit => {
                    self._tray = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                Event::Open(queue, target, clipboard) => {
                    if platform::valid_target(target, self.owner) {
                        self.target = target;
                    }
                    self.backend.visible.store(true, Ordering::Relaxed);
                    // 呼出剪贴板时始终回到剪贴板页：退出启动器、JSON、时间戳等页面。
                    self.launcher.active = false;
                    self.timestamp.active = false;
                    self.json.active = false;
                    if queue {
                        self.calculator.leave();
                    }
                    if !self.calculator.active {
                        self.query.clear();
                        self.group.clear();
                        self.filter = if queue { "queue" } else { "all" }.into();
                        self.selected = if queue {
                            String::new()
                        } else {
                            clipboard.unwrap_or_default()
                        };
                        self.history_top = true;
                        self.history_up_distance = 0.;
                        self.scroll_selected = false;
                    }
                    self.calculator.focus = true;
                    self.settings_open = false;
                    self.confirm = None;
                    self.dirty = true;
                    self.focus_search = true;
                    self.was_focused = false;
                    // 呼出剪贴板时预览回到持久偏好；图片选中导致的临时展开不会延续。
                    self.preview_open = self.preview_default;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && self._tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        if !self.backend.visible.load(Ordering::Relaxed) {
            return;
        }
        if !self.launcher.active {
            resize_border(ctx);
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        // 面板激活时生效的 JSON / 时间戳快捷键，在所有页面的按键处理之前消费。
        if focused
            && !self.settings_open
            && self.confirm.is_none()
            && self.enlarged_image.is_none()
            && !self.launcher.managing()
            && self.consume_page_shortcuts(ctx)
        {
            ctx.request_repaint();
            return;
        }
        if self.launcher.active
            && self.was_focused
            && !focused
            && !self.pinned
            && !self.launcher.managing()
        {
            self.hide(ctx);
            return;
        }
        if self.was_focused
            && !focused
            && !self.pinned
            && !self.calculator.active
            && !self.json.active
            && !self.timestamp.active
            && !self.launcher.active
            && !self.settings_open
            && self.confirm.is_none()
        {
            self.hide(ctx);
        }
        self.was_focused = focused;
        if self.launcher.active {
            let original = self.json_window.take_original();
            self.launcher.enter_window(ctx, self.owner, original);
        } else if self.launcher.restore_window(ctx) {
            ctx.request_repaint();
            return;
        }
        if !self.launcher.active {
            self.json_window.update(ctx, self.json.active, self.owner);
        }
        if self.launcher.active {
            self.launcher_page(ctx);
            return;
        }
        if self.timestamp.active {
            self.timestamp_page(ctx);
            return;
        }
        if self.json.active {
            self.json_page(ctx);
            return;
        }
        if self.calculator.active {
            self.calculator_page(ctx);
            if self.settings_open {
                self.capture_hotkey(ctx);
            } else {
                self.hotkey_capture = None;
            }
            let settings_were_open = self.settings_open;
            self.dialogs(ctx);
            if settings_were_open && !self.settings_open {
                self.calculator.focus = true;
            }
            return;
        }
        self.refresh();
        let busy = self.backend.busy.load(Ordering::Relaxed);
        if !self.settings_open
            && self.confirm.is_none()
            && self.enlarged_image.is_none()
            && self.consume_find_shortcut(ctx)
        {
            self.focus_search = true;
        }
        if !self.settings_open
            && self.confirm.is_none()
            && self.enlarged_image.is_none()
            && self.hotkey_capture.is_none()
            && !self.ime_composing
            && consume_local_shortcut(ctx, "Ctrl+P")
        {
            self.set_preview(!self.preview_open);
        }
        let mut navigation_active = false;
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ctx, |ui| {
            let p = palette(self.dark, self.theme);
            // Frameless window: the top edge doubles as the drag strip.
            let drag = ui.allocate_response(
                Vec2::new(ui.available_width(), 2.),
                egui::Sense::drag(),
            );
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.add_space(6.);
            let command_mode = self.query.starts_with(['>', '＞']);
            let search = ui
                .horizontal(|ui| {
                    ui.label(
                        mono(if command_mode { ">" } else { "›" })
                            .size(16.)
                            .strong()
                            .color(p.accent),
                    );
                    let icons_w = 3. * 28. + 2. * ui.spacing().item_spacing.x;
                    let edit_w = (ui.available_width() - icons_w - 10.).max(80.);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("搜索剪贴板…　> 工具 · = 计算")
                            .font(egui::TextStyle::Body)
                            .desired_width(edit_w)
                            .char_limit(4096)
                            .frame(false),
                    );
                    if ghost_icon(ui, &p, "pin", self.pinned, if self.pinned { "取消固定 · 点击窗口外自动收起" } else { "固定窗口 · 点击窗口外保持显示" }).clicked() {
                        self.pinned = !self.pinned;
                    }
                    if ghost_icon(ui, &p, if self.dark { "sun" } else { "moon" }, false, if self.dark { "切换为浅色外观" } else { "切换为深色外观" }).clicked() {
                        self.dark = !self.dark;
                        apply_theme(ctx, self.dark, self.theme);
                        if let Ok(dir) = backend::data_dir() {
                            UiState::of(self).save(&dir);
                        }
                    }
                    if ghost_icon(ui, &p, "settings", self.settings_open, "偏好设置").clicked() {
                        self.open_settings(SettingsSection::General);
                    }
                    edit
                })
                .inner;
            if search.changed() {
                let mut composing = self.ime_composing;
                ctx.input(|i| { for event in &i.events {
                    match event {
                        egui::Event::Ime(egui::ImeEvent::Preedit(text)) => composing = !text.is_empty(),
                        egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) => composing = false,
                        _ => {}
                    }
                }});
                if !composing {
                    let pasted = ctx.input(|i| i.events.iter().find_map(|e| match e {
                        egui::Event::Paste(text) if crate::calculator::expression_query(text).is_some()
                            && crate::calculator::expression_query(&self.query).is_some() => Some(text.clone()),
                        _ => None,
                    }));
                    if let Some(text) = pasted { self.query = text; }
                    if let Some(expression) = crate::calculator::expression_query(&self.query) {
                        self.calculator.enter(expression);
                        self.query.clear();
                        ctx.request_repaint();
                        return;
                    }
                }
                self.dirty = true;
                self.command_selected = 0;
            }
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            navigation_active = search.has_focus() || !ctx.wants_keyboard_input();
            ui.add_space(4.);
            if !command_mode {
            let previous_row_height = ui.spacing().interact_size.y;
            ui.spacing_mut().interact_size.y = 24.;
            ui.horizontal(|ui| {
                let organizer = self.backend.store.lock().unwrap().organizer_view();
                {
                    // Categories and groups share one layout and baseline.
                    // Overflow scrolls together while the queue chip stays fixed.
                    let group_width = (ui.available_width() - 100.).max(48.);
                    egui::ScrollArea::horizontal()
                        .id_salt("group-filter")
                        .max_height(24.)
                        .auto_shrink([true, false])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .max_width(group_width)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.set_height(24.);
                                for (id, label) in [
                                    ("all", "全部"), ("text", "文字"),
                                    ("image", "图片"), ("pinned", "已保存"),
                                ] {
                                    if pill(ui, &p, label, self.filter == id).clicked() && self.filter != id {
                                        self.filter = id.into();
                                        self.dirty = true;
                                    }
                                }
                                for g in &organizer.groups {
                                    let selected = self.group == g.id;
                                    if pill(ui, &p, &g.name, selected).clicked() {
                                        if selected {
                                            self.group.clear();
                                        } else {
                                            self.group = g.id.clone();
                                        }
                                        self.dirty = true;
                                    }
                                }
                            });
                        });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = organizer.queue.len();
                    if pill(ui, &p, &format!("队列 {count}"), self.filter == "queue").clicked() {
                        self.filter = if self.filter == "queue" { "all" } else { "queue" }.into();
                        self.query.clear();
                        self.group.clear();
                        self.dirty = true;
                    }
                });
            });
            ui.spacing_mut().interact_size.y = previous_row_height;
            }
            if !self.hotkey_error.is_empty() {
                ui.colored_label(p.error, mono(&self.hotkey_error).size(11.));
            }
            if self.filter == "queue" && !command_mode {
                ui.add_space(2.);
                egui::Frame::default()
                    .fill(p.card_selected)
                    .stroke(egui::Stroke::new(1., p.hairline))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let count =
                                    self.backend.store.lock().unwrap().organizer.queue.len();
                                ui.label(
                                    mono(format!("{count} 条待粘贴"))
                                        .size(11.5)
                                        .strong()
                                        .color(p.accent),
                                );
                                if ui.add_enabled(count > 0, egui::Button::new("粘贴下一条")).clicked() {
                                    self.apply(true, true, false);
                                }
                                if ui.add_enabled(count > 0, egui::Button::new("合并粘贴")).clicked() {
                                    self.apply(true, false, true);
                                }
                                if ui.button("恢复上次").clicked() {
                                    self.operation(|s| s.restore_queue());
                                }
                                if ui.button("清空排列").clicked() {
                                    self.operation(|s| s.queue(vec![]));
                                }
                                if pill(ui, &p, "返回添加", false).clicked() {
                                    self.filter = "all".into();
                                    self.dirty = true;
                                }
                            });
                            ui.label(RichText::new("点记录左侧 + 加入；↑ ↓ 调整顺序。下一条按顺序取出，合并粘贴仅支持文字。").size(11.).color(p.text_dim));
                        });
                    });
            }
            ui.add_space(2.);
        });
        // Resolve changed filters before any keyboard or pointer action this frame.
        if self.calculator.active {
            return;
        }
        self.refresh();
        let modal = self.settings_open || self.confirm.is_some() || self.enlarged_image.is_some();
        let ime = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Ime(_))));
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Ime(event) = event {
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
        if !modal && !busy && !ime && !self.ime_composing && navigation_active {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                if self.filter == "queue" {
                    self.filter = "all".into();
                    self.dirty = true;
                } else if self.query.is_empty() {
                    self.hide(ctx);
                } else {
                    self.query.clear();
                    self.dirty = true;
                }
            }
            let delta = ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowDown) {
                    1
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    -1
                } else if i.key_pressed(egui::Key::PageDown) {
                    5
                } else if i.key_pressed(egui::Key::PageUp) {
                    -5
                } else {
                    0
                }
            });
            if self.query.starts_with(['>', '＞']) {
                // 命令模式：↑↓ 在工具列表中移动，Enter 执行。
                let commands = self.command_items();
                if delta != 0 && !commands.is_empty() {
                    let index = self.command_selected as i32;
                    self.command_selected =
                        (index + delta).clamp(0, commands.len() as i32 - 1) as usize;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(command) = commands.get(self.command_selected) {
                        let id = command.id;
                        self.run_command(id);
                    }
                }
            } else {
                if delta != 0 && !self.items.is_empty() {
                    let index = self
                        .items
                        .iter()
                        .position(|e| e.id == self.selected)
                        .unwrap_or(0) as i32;
                    self.selected = self.items
                        [(index + delta).clamp(0, self.items.len() as i32 - 1) as usize]
                        .id
                        .clone();
                    self.scroll_selected = true;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let modifiers = ctx.input(|i| i.modifiers);
                    self.apply(
                        !modifiers.shift,
                        self.filter == "queue" && !modifiers.ctrl,
                        modifiers.ctrl && self.filter == "queue",
                    );
                }
            }
        }
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ctx, |ui| {
                let p = palette(self.dark, self.theme);
                ui.painter().hline(
                    ui.available_rect_before_wrap().x_range(),
                    ui.cursor().top() - 6.,
                    egui::Stroke::new(1., p.hairline),
                );
                ui.horizontal(|ui| {
                    let enabled = self.backend.store.lock().unwrap().settings.enabled;
                    let status = egui::Button::new(
                        RichText::new(if enabled {
                            "● 记录中"
                        } else {
                            "○ 已暂停"
                        })
                        .size(10.5)
                        .color(if enabled {
                            p.accent
                        } else {
                            p.text_dim
                        }),
                    )
                    .frame(false);
                    if ui
                        .add(status)
                        .on_hover_text(if enabled {
                            "点击暂停记录"
                        } else {
                            "点击恢复记录"
                        })
                        .clicked()
                    {
                        self.operation(|s| {
                            let mut c = s.settings.clone();
                            c.enabled = !c.enabled;
                            s.save_settings(c)
                        });
                    }
                    ui.label(
                        mono(format!("· {} 条", self.items.len()))
                            .size(10.5)
                            .color(p.text_dim),
                    );
                    let backend_status = self.backend.status.lock().unwrap().clone();
                    if !backend_status.is_empty() {
                        ui.label(mono(backend_status).size(10.5).color(p.text_dim));
                    }
                    if busy {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.;
                        for (key, label) in [
                            ("Esc", "关闭"),
                            (">", "工具"),
                            ("Ctrl+P", "预览"),
                            ("⇧⏎", "复制"),
                            ("⏎", "粘贴"),
                            ("↑↓", "选择"),
                        ] {
                            ui.label(RichText::new(label).size(10.).color(p.text_dim));
                            kbd(ui, &p, key);
                            ui.add_space(6.);
                        }
                    });
                });
            });
        // 预览数据随选中即时加载；抽屉在 CentralPanel 内按需绘制。
        self.preview(ctx);
        if self.smoke.is_some()
            && std::env::args().any(|a| a == "--enlarge")
            && self.texture.is_some()
            && self.enlarged_image.is_none()
        {
            self.enlarge_image(ctx);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!busy && !modal, |ui| {
                    if self.query.starts_with(['>', '＞']) {
                        self.command_page(ui);
                        return;
                    }
                    let drawer_w = if self.preview_open { 260. } else { 0. };
                    let list_w = (ui.available_width() - drawer_w).max(120.);
                    // 注意：横向布局里 available_height() 不可靠（cursor 高度为 0），先取好。
                    let panel_h = ui.available_height();
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(list_w, panel_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                if self.items.is_empty() {
                                    ui.add_space(80.);
                                    ui.centered_and_justified(|ui| {
                                        ui.vertical(|ui| {
                                            ui.centered_and_justified(|ui| {
                                                ui.label(
                                                    mono(if self.filter == "queue" {
                                                        "还没有待粘贴的内容"
                                                    } else {
                                                        "—— 没有匹配的记录 ——"
                                                    })
                                                    .size(13.)
                                                    .color(palette(self.dark, self.theme).text_dim),
                                                );
                                            });
                                            ui.add_space(6.);
                                            ui.centered_and_justified(|ui| {
                                                ui.label(
                                                    RichText::new(if self.filter == "queue" {
                                                        "点击「返回添加」，再点记录左侧 + 加入排列"
                                                    } else {
                                                        "开启记录后，在其他应用复制内容"
                                                    })
                                                    .size(11.5)
                                                    .color(palette(self.dark, self.theme).text_dim),
                                                );
                                            });
                                        });
                                    });
                                    return;
                                }
                                if self.history_up_distance > 140.
                                    && self.history_offset > ROW_STEP * 3.
                                {
                                    ui.horizontal(|ui| {
                                        if ui.button("↑ 回到顶部").clicked() {
                                            self.history_top = true;
                                            self.history_up_distance = 0.;
                                            self.scroll_selected = false;
                                        }
                                    });
                                }
                                let mut area = egui::ScrollArea::vertical()
                                    .id_salt("history")
                                    .auto_shrink([false, false]);
                                if self.scroll_selected {
                                    if let Some(index) =
                                        self.items.iter().position(|e| e.id == self.selected)
                                    {
                                        area = area.vertical_scroll_offset(index as f32 * ROW_STEP);
                                    }
                                    self.scroll_selected = false;
                                }
                                if self.history_top {
                                    area = area.vertical_scroll_offset(0.);
                                    self.history_top = false;
                                }
                                let items = std::mem::take(&mut self.items);
                                self.thumb_budget = THUMB_BUDGET;
                                let history_scroll =
                                    area.show_rows(ui, ROW_HEIGHT, items.len(), |ui, range| {
                                        let p = palette(self.dark, self.theme);
                                        for index in range {
                                            let item = &items[index];
                                            ui.horizontal(|ui| {
                                                let in_queue = self.filter == "queue";
                                                let queued = self
                                                    .backend
                                                    .store
                                                    .lock()
                                                    .unwrap()
                                                    .organizer
                                                    .queue
                                                    .contains(&item.id);
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            mono(if queued { "−" } else { "+" })
                                                                .color(p.accent),
                                                        )
                                                        .fill(if queued {
                                                            p.accent_soft
                                                        } else {
                                                            Color32::TRANSPARENT
                                                        })
                                                        .min_size(Vec2::new(24., 24.)),
                                                    )
                                                    .on_hover_text(if queued {
                                                        "已加入排列，点击移除"
                                                    } else {
                                                        "加入排列粘贴"
                                                    })
                                                    .clicked()
                                                {
                                                    let id = item.id.clone();
                                                    self.operation(|s| {
                                                        let mut q = s.organizer.queue.clone();
                                                        if q.contains(&id) {
                                                            q.retain(|v| v != &id);
                                                        } else {
                                                            q.push(id);
                                                        }
                                                        s.queue(q)
                                                    });
                                                }
                                                let width = ui.available_width()
                                                    - if in_queue { 64. } else { 0. };
                                                let (rect, response) = ui.allocate_exact_size(
                                                    Vec2::new(width, ROW_HEIGHT),
                                                    egui::Sense::click(),
                                                );
                                                let selected = item.id == self.selected;
                                                let hover = ui.ctx().animate_bool_with_time(
                                                    egui::Id::new(("row-hover", &item.id)),
                                                    response.hovered(),
                                                    0.08,
                                                );
                                                let fill = if selected {
                                                    p.card_selected
                                                } else {
                                                    mix(p.card, p.card_hover, hover)
                                                };
                                                let border = if selected {
                                                    p.accent
                                                } else {
                                                    mix(p.hairline, p.accent, hover * 0.6)
                                                };
                                                // Paint the card, then draw contents on top in a child UI.
                                                let corner = CornerRadius::same(5);
                                                ui.painter().add(egui::Shape::rect_filled(
                                                    rect, corner, fill,
                                                ));
                                                if selected {
                                                    ui.painter().add(egui::Shape::rect_filled(
                                                        egui::Rect::from_min_size(
                                                            rect.min + egui::vec2(1.5, 1.5),
                                                            egui::vec2(3., rect.height() - 3.),
                                                        ),
                                                        CornerRadius {
                                                            nw: 2,
                                                            ne: 0,
                                                            se: 0,
                                                            sw: 2,
                                                        },
                                                        p.accent,
                                                    ));
                                                }
                                                ui.painter().add(egui::Shape::rect_stroke(
                                                    rect,
                                                    corner,
                                                    egui::Stroke::new(1., border),
                                                    egui::StrokeKind::Inside,
                                                ));
                                                let mut card = ui.new_child(
                                                    egui::UiBuilder::new()
                                                        .id_salt(egui::Id::new(&item.id))
                                                        .max_rect(rect.shrink2(Vec2::new(12., 6.))),
                                                );
                                                card.horizontal_top(|ui| {
                                                    if item.kind == "image" {
                                                        self.row_thumbnail(ui, item);
                                                    }
                                                    ui.vertical(|ui| {
                                                        ui.add_space(1.);
                                                        ui.horizontal(|ui| {
                                                            if item.pinned {
                                                                ui.label(
                                                                    RichText::new("★")
                                                                        .color(p.star),
                                                                );
                                                            }
                                                            let summary = item
                                                                .preview
                                                                .split_whitespace()
                                                                .collect::<Vec<_>>()
                                                                .join(" ");
                                                            let mut title: String =
                                                                summary.chars().take(75).collect();
                                                            if summary.chars().count() > 75 {
                                                                title.push('…');
                                                            }
                                                            ui.add(
                                                                egui::Label::new(
                                                                    RichText::new(title)
                                                                        .strong()
                                                                        .color(p.text),
                                                                )
                                                                .truncate(),
                                                            );
                                                        });
                                                        ui.horizontal(|ui| {
                                                            let kind = match item.kind.as_str() {
                                                                "image" => "IMG",
                                                                "link" => "URL",
                                                                _ => "TXT",
                                                            };
                                                            kind_tag(ui, &p, kind);
                                                            if !item.source.is_empty() {
                                                                ui.label(
                                                                    RichText::new(&item.source)
                                                                        .small()
                                                                        .color(p.text_dim),
                                                                );
                                                            }
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.label(
                                                                        mono(time_label(
                                                                            item.copied_at,
                                                                        ))
                                                                        .size(10.5)
                                                                        .color(p.text_dim),
                                                                    );
                                                                    if selected {
                                                                        ui.add_space(8.);
                                                                        ui.label(
                                                                            mono("⏎ 粘贴")
                                                                                .size(10.5)
                                                                                .strong()
                                                                                .color(p.accent),
                                                                        );
                                                                    }
                                                                },
                                                            );
                                                        });
                                                    });
                                                });
                                                if response.clicked() {
                                                    self.selected = item.id.clone();
                                                }
                                                if response.double_clicked() {
                                                    self.selected = item.id.clone();
                                                    self.apply(true, false, false);
                                                }
                                                if in_queue {
                                                    for (label, delta) in [("↑", -1isize), ("↓", 1)]
                                                    {
                                                        if ui
                                                            .small_button(
                                                                mono(label).color(p.text_dim),
                                                            )
                                                            .clicked()
                                                        {
                                                            let id = item.id.clone();
                                                            self.operation(|s| {
                                                                let mut q =
                                                                    s.organizer.queue.clone();
                                                                if let Some(i) =
                                                                    q.iter().position(|v| v == &id)
                                                                {
                                                                    let j = i as isize + delta;
                                                                    if j >= 0
                                                                        && (j as usize) < q.len()
                                                                    {
                                                                        q.swap(i, j as usize);
                                                                    }
                                                                }
                                                                s.queue(q)
                                                            });
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                    });
                                let offset = history_scroll.state.offset.y;
                                let delta = self.history_offset - offset;
                                if delta > 0. {
                                    self.history_up_distance += delta;
                                } else if delta < -1. {
                                    self.history_up_distance = 0.;
                                }
                                if offset < 1. {
                                    self.history_up_distance = 0.;
                                }
                                self.history_offset = offset;
                                self.items = items;
                            },
                        );
                        if self.preview_open {
                            self.preview_drawer(ui, panel_h);
                        }
                    });
                });
            });
        if self.settings_open {
            self.capture_hotkey(ctx);
        } else {
            self.hotkey_capture = None;
        }
        self.dialogs(ctx);
    }
}

impl App {
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(texture) = self.enlarged_image.clone() {
            let p = palette(self.dark, self.theme);
            let mut close = false;
            let response = egui::Modal::new(egui::Id::new("image-viewer"))
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(p.panel)
                        .stroke(egui::Stroke::new(1., p.border))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ctx, |ui| {
                    let size = Vec2::new(
                        (ctx.content_rect().width() - 64.).max(120.),
                        (ctx.content_rect().height() - 120.).max(100.),
                    );
                    ui.set_width(size.x);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("图片预览").strong().color(p.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, &p, "close", false, "关闭预览 · Esc").clicked()
                            {
                                close = true;
                            }
                            if pill(ui, &p, "100%", self.image_actual_size).clicked() {
                                self.image_actual_size = true;
                            }
                            if pill(ui, &p, "适应窗口", !self.image_actual_size).clicked() {
                                self.image_actual_size = false;
                            }
                        });
                    });
                    ui.add_space(6.);
                    egui::ScrollArea::both()
                        .id_salt("enlarged-image-scroll")
                        .max_height(size.y)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let original = texture.size_vec2();
                            let scale = (size.x / original.x).min(size.y / original.y).min(1.);
                            let display = if self.image_actual_size {
                                original
                            } else {
                                original * scale
                            };
                            ui.add(egui::Image::new(&texture).fit_to_exact_size(display));
                        });
                    ui.label(
                        RichText::new("点击外部或按 Esc 关闭；100% 模式可滚动查看细节")
                            .size(11.)
                            .color(p.text_dim),
                    );
                });
            if close || response.should_close() {
                self.enlarged_image = None;
            }
            return;
        }
        if self.settings_open {
            let mut open = true;
            egui::Window::new("偏好设置")
                .title_bar(false)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(palette(self.dark, self.theme).panel)
                        .stroke(egui::Stroke::new(1., palette(self.dark, self.theme).border))
                        .corner_radius(CornerRadius::same(APP_CORNER_RADIUS))
                        .shadow(egui::Shadow::NONE)
                        .inner_margin(egui::Margin::same(0)),
                )
                .resizable(false)
                .fixed_size([800., 560.])
                .show(ctx, |ui| {
                    let p = palette(self.dark, self.theme);
                    let height = ui.available_height();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let (nav_rect, _) = ui
                            .allocate_exact_size(Vec2::new(190., height), egui::Sense::hover());
                        ui.painter().rect_filled(
                            nav_rect,
                            CornerRadius {
                                nw: APP_CORNER_RADIUS,
                                ne: 0,
                                se: 0,
                                sw: APP_CORNER_RADIUS,
                            },
                            p.card,
                        );
                        ui.painter().vline(
                            nav_rect.right(),
                            nav_rect.y_range(),
                            egui::Stroke::new(1., p.hairline),
                        );
                        let mut nav = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt("settings-nav")
                                .max_rect(nav_rect)
                                .layout(egui::Layout::top_down(egui::Align::LEFT)),
                        );
                        egui::Frame::default()
                            .inner_margin(egui::Margin {
                                left: 10,
                                right: 10,
                                top: 14,
                                bottom: 10,
                            })
                            .show(&mut nav, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (back_rect, back) = ui.allocate_exact_size(
                                        Vec2::splat(22.),
                                        egui::Sense::click(),
                                    );
                                    let hovering = back.hovered();
                                    if hovering {
                                        ui.painter().rect_filled(
                                            back_rect,
                                            CornerRadius::same(6),
                                            p.card_hover,
                                        );
                                    }
                                    let c = back_rect.center();
                                    let arrow = egui::Stroke::new(
                                        1.6,
                                        if hovering { p.accent } else { p.text_dim },
                                    );
                                    ui.painter().line_segment(
                                        [c + egui::vec2(4.5, 0.), c + egui::vec2(-3.5, 0.)],
                                        arrow,
                                    );
                                    ui.painter().line_segment(
                                        [c + egui::vec2(-3.5, 0.), c + egui::vec2(0., -3.5)],
                                        arrow,
                                    );
                                    ui.painter().line_segment(
                                        [c + egui::vec2(-3.5, 0.), c + egui::vec2(0., 3.5)],
                                        arrow,
                                    );
                                    if back.on_hover_text("返回剪贴板").clicked() {
                                        open = false;
                                    }
                                    ui.add_space(8.);
                                    let (mark, _) = ui.allocate_exact_size(
                                        Vec2::new(16., 16.),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter()
                                        .rect_filled(mark, CornerRadius::same(4), p.accent);
                                    ui.painter().rect_filled(
                                        mark.shrink(5.),
                                        CornerRadius::same(2),
                                        p.card,
                                    );
                                    ui.add_space(2.);
                                    ui.label(
                                        RichText::new("偏好设置")
                                            .size(13.)
                                            .strong()
                                            .color(p.text),
                                    );
                                    ui.add_space(5.);
                                    ui.label(
                                        RichText::new("Clibo").size(10.).color(p.text_dim),
                                    );
                                });
                                ui.add_space(12.);
                                egui::Frame::default()
                                    .fill(p.input)
                                    .stroke(egui::Stroke::new(1., p.border))
                                    .corner_radius(CornerRadius::same(7))
                                    .inner_margin(egui::Margin::symmetric(9, 2))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings_query,
                                            )
                                            .hint_text("搜索设置…")
                                            .desired_width(f32::INFINITY)
                                            .frame(false),
                                        );
                                    });
                                ui.add_space(10.);
                                for section in SettingsSection::ALL {
                                    if !section.matches(&self.settings_query) {
                                        continue;
                                    }
                                    if settings_nav_item(
                                        ui,
                                        &p,
                                        section,
                                        self.settings_section == section,
                                    )
                                    .clicked()
                                    {
                                        self.settings_section = section;
                                        self.hotkey_capture = None;
                                    }
                                }
                                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                                    ui.add_space(4.);
                                    ui.label(
                                        mono("Clibo · 本地加密存储")
                                            .size(10.)
                                            .color(p.text_dim.gamma_multiply(0.7)),
                                    );
                                });
                            });
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width());
                            ui.set_height(height);
                            egui::ScrollArea::vertical()
                                .id_salt("settings-body")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                )
                                .auto_shrink([false, false])
                                .max_height((height - 38.).max(120.))
                                .show(ui, |ui| {
                                    egui::Frame::default()
                                        .inner_margin(egui::Margin {
                                            left: 22,
                                            right: 22,
                                            top: 20,
                                            bottom: 8,
                                        })
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.vertical(|ui| {
                                                    ui.set_width(
                                                        (ui.available_width() - 40.).max(200.),
                                                    );
                                                    ui.label(
                                                        RichText::new(
                                                            self.settings_section.label(),
                                                        )
                                                        .size(16.)
                                                        .strong()
                                                        .color(p.text),
                                                    );
                                                    ui.label(
                                                        RichText::new(match self.settings_section {
                                                            SettingsSection::General => {
                                                                "启动、外观与记录开关"
                                                            }
                                                            SettingsSection::Clipboard => {
                                                                "采集策略与历史保留"
                                                            }
                                                            SettingsSection::Launcher => {
                                                                "搜索范围与指令前缀"
                                                            }
                                                            SettingsSection::Hotkeys => {
                                                                "点击「重录」后按下新组合键；需包含 Ctrl 或 Alt，Windows 不支持 Win 键"
                                                            }
                                                            SettingsSection::Data => {
                                                                "本地加密存储，备份保存在数据目录"
                                                            }
                                                        })
                                                        .size(11.5)
                                                        .color(p.text_dim),
                                                    );
                                                });
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        if icon_button(
                                                            ui,
                                                            &p,
                                                            "close",
                                                            false,
                                                            "关闭设置",
                                                        )
                                                        .clicked()
                                                        {
                                                            open = false;
                                                        }
                                                    },
                                                );
                                            });
                                            ui.add_space(16.);
                                            match self.settings_section {
                            SettingsSection::General => {
                                settings_card(ui, &p, "启动与外观", |ui| {
                                    #[cfg(windows)]
                                    {
                                        setting_row(
                                            ui,
                                            &p,
                                            "开机自启",
                                            "登录 Windows 后在托盘后台运行，移动程序后需重新开启",
                                            |ui| match self.autostart.clone() {
                                                Ok(mut enabled) => {
                                                    if toggle(ui, &p, &mut enabled, "立即生效") {
                                                        let result =
                                                            crate::autostart::set_enabled(enabled);
                                                        self.backend.report(result);
                                                        self.autostart =
                                                            crate::autostart::enabled();
                                                    }
                                                }
                                                Err(error) => {
                                                    ui.colored_label(p.error, error);
                                                }
                                            },
                                        );
                                        card_divider(ui, &p);
                                    }
                                    setting_row(ui, &p, "主题", "面板、启动器与设置窗口共用", |ui| {
                                        for theme in [Theme::Blue, Theme::Amber] {
                                            if pill(ui, &p, theme.label(), self.theme == theme).clicked()
                                                && self.theme != theme
                                            {
                                                self.theme = theme;
                                                apply_theme(ui.ctx(), self.dark, self.theme);
                                                if let Ok(dir) = backend::data_dir() {
                                                    UiState::of(self).save(&dir);
                                                }
                                            }
                                            ui.add_space(6.);
                                        }
                                    });
                                    card_divider(ui, &p);
                                    setting_row(ui, &p, "外观", "", |ui| {
                                        if pill(ui, &p, "浅色", !self.dark).clicked()
                                            && self.dark
                                        {
                                            self.dark = false;
                                            apply_theme(ui.ctx(), self.dark, self.theme);
                                            if let Ok(dir) = backend::data_dir() {
                                                UiState::of(self).save(&dir);
                                            }
                                        }
                                        ui.add_space(6.);
                                        if pill(ui, &p, "深色", self.dark).clicked()
                                            && !self.dark
                                        {
                                            self.dark = true;
                                            apply_theme(ui.ctx(), self.dark, self.theme);
                                            if let Ok(dir) = backend::data_dir() {
                                                UiState::of(self).save(&dir);
                                            }
                                        }
                                    });
                                    card_divider(ui, &p);
                                    setting_row(
                                        ui,
                                        &p,
                                        "预览面板",
                                        "关闭后不再默认展开预览抽屉，仍可用 Ctrl+P 切换",
                                        |ui| {
                                            let mut value = self.preview_open;
                                            if toggle(ui, &p, &mut value, "") {
                                                self.set_preview(value);
                                            }
                                        },
                                    );
                                });
                                settings_card(ui, &p, "记录", |ui| {
                                    setting_row(
                                        ui,
                                        &p,
                                        "记录剪贴板",
                                        "暂停后不再采集新内容，已有历史保留",
                                        |ui| {
                                            toggle(ui, &p, &mut self.settings.enabled, "");
                                        },
                                    );
                                });
                            }
                            SettingsSection::Clipboard => {
                                settings_card(ui, &p, "采集", |ui| {
                                    setting_row(ui, &p, "记录图片", "关闭后仅采集文本", |ui| {
                                        toggle(ui, &p, &mut self.settings.capture_images, "");
                                    });
                                    card_divider(ui, &p);
                                    setting_row(
                                        ui,
                                        &p,
                                        "跳过疑似密钥",
                                        "检测到密码、Token 格式时自动忽略，不入库",
                                        |ui| {
                                            toggle(
                                                ui,
                                                &p,
                                                &mut self.settings.detect_sensitive,
                                                "",
                                            );
                                        },
                                    );
                                });
                                settings_card(ui, &p, "历史", |ui| {
                                    setting_row(ui, &p, "历史上限", "100 – 2000 条", |ui| {
                                        ui.horizontal(|ui| {
                                            stepper(
                                                ui,
                                                &p,
                                                &mut self.settings.max_items,
                                                100usize,
                                                2000usize,
                                                50usize,
                                            );
                                            ui.label(mono("条").size(10.5).color(p.text_dim));
                                        });
                                    });
                                    card_divider(ui, &p);
                                    setting_row(
                                        ui,
                                        &p,
                                        "保留天数",
                                        "1 – 365 天，超期自动清理",
                                        |ui| {
                                            ui.horizontal(|ui| {
                                                stepper(
                                                    ui,
                                                    &p,
                                                    &mut self.settings.retention_days,
                                                    1i64,
                                                    365i64,
                                                    1i64,
                                                );
                                                ui.label(mono("天").size(10.5).color(p.text_dim));
                                            });
                                        },
                                    );
                                    card_divider(ui, &p);
                                    setting_row(ui, &p, "JSON 历史保留", "10 – 500 条", |ui| {
                                        ui.horizontal(|ui| {
                                            stepper(
                                                ui,
                                                &p,
                                                &mut self.settings.json_history_max_items,
                                                10usize,
                                                500usize,
                                                10usize,
                                            );
                                            ui.label(mono("条").size(10.5).color(p.text_dim));
                                        });
                                    });
                                });
                                settings_card(ui, &p, "排除应用", |ui| {
                                    egui::Frame::default()
                                        .inner_margin(egui::Margin::symmetric(14, 10))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.add(
                                                egui::TextEdit::multiline(&mut self.excluded)
                                                    .desired_width(f32::INFINITY)
                                                    .desired_rows(4),
                                            );
                                            ui.add_space(6.);
                                            ui.label(
                                                RichText::new(
                                                    "每行一个进程名，这些应用的复制内容不会被记录",
                                                )
                                                .size(10.5)
                                                .color(p.text_dim),
                                            );
                                        });
                                });
                                settings_card(ui, &p, "分组", |ui| {
                                    let groups = self
                                        .backend
                                        .store
                                        .lock()
                                        .unwrap()
                                        .organizer
                                        .groups
                                        .clone();
                                    for (index, g) in groups.iter().enumerate() {
                                        if index > 0 {
                                            card_divider(ui, &p);
                                        }
                                        egui::Frame::default()
                                            .inner_margin(egui::Margin::symmetric(14, 8))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(&g.name)
                                                            .size(12.3)
                                                            .color(p.text),
                                                    );
                                                    let count = self
                                                        .backend
                                                        .store
                                                        .lock()
                                                        .unwrap()
                                                        .entries
                                                        .iter()
                                                        .filter(|e| {
                                                            e.view.group_ids.contains(&g.id)
                                                        })
                                                        .count();
                                                    ui.label(
                                                        mono(format!("{count} 条"))
                                                            .size(10.5)
                                                            .color(p.text_dim),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            if danger_pill(ui, &p, "解散")
                                                                .clicked()
                                                            {
                                                                self.operation(|s| {
                                                                    s.group(&g.id, "", true)
                                                                });
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                    }
                                    if !groups.is_empty() {
                                        card_divider(ui, &p);
                                    }
                                    egui::Frame::default()
                                        .inner_margin(egui::Margin::symmetric(14, 8))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.group_name,
                                                    )
                                                    .hint_text("新分组名称")
                                                    .char_limit(40)
                                                    .desired_width(180.),
                                                );
                                                if pill(ui, &p, "创建", false).clicked() {
                                                    let name = self.group_name.clone();
                                                    self.operation(|s| s.group("", &name, false));
                                                    self.group_name.clear();
                                                }
                                            });
                                            ui.add_space(6.);
                                            ui.label(
                                                RichText::new(
                                                    "条目归入分组：主面板选中条目 → 预览抽屉 → 备注与分组",
                                                )
                                                .size(10.5)
                                                .color(p.text_dim),
                                            );
                                        });
                                });
                            }
                            SettingsSection::Launcher => {
                                settings_card(ui, &p, "搜索范围", |ui| {
                                    setting_row(
                                        ui,
                                        &p,
                                        "搜索浏览器书签",
                                        "Chrome / Edge / Brave / Firefox",
                                        |ui| {
                                            toggle(
                                                ui,
                                                &p,
                                                &mut self.settings.launcher_bookmarks,
                                                "",
                                            );
                                        },
                                    );
                                    card_divider(ui, &p);
                                    egui::Frame::default()
                                        .inner_margin(egui::Margin::symmetric(14, 10))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(
                                                RichText::new(
                                                    "搜索目录 · 每行一个；留空时使用系统默认目录",
                                                )
                                                .size(11.)
                                                .color(p.text_dim),
                                            );
                                            ui.add_space(6.);
                                            ui.add(
                                                egui::TextEdit::multiline(
                                                    &mut self.launcher_roots,
                                                )
                                                .desired_width(f32::INFINITY)
                                                .desired_rows(4)
                                                .hint_text("例如 C:\\Work\\Projects"),
                                            );
                                            ui.add_space(4.);
                                            ui.label(
                                                RichText::new("保存后会自动重新建立启动器索引。")
                                                    .size(10.5)
                                                    .color(p.text_dim),
                                            );
                                        });
                                });
                                settings_card(ui, &p, "搜索指令前缀", |ui| {
                                    egui::Frame::default()
                                        .inner_margin(egui::Margin::symmetric(14, 10))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(
                                                RichText::new(format!(
                                                    "例如：{} wx 只搜索应用。下面的指令都可以修改。",
                                                    self.settings.launcher_search_prefixes.apps
                                                ))
                                                .size(11.)
                                                .color(p.text_dim),
                                            );
                                            ui.add_space(8.);
                                egui::Grid::new("launcher-search-prefixes")
                                    .num_columns(3)
                                    .spacing(Vec2::new(14., 8.))
                                    .show(ui, |ui| {
                                        ui.label("应用");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.apps,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("只搜索应用");
                                        ui.end_row();

                                        ui.label("文件");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.files,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("只搜索文件与文件夹");
                                        ui.end_row();

                                        ui.label("书签");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.bookmarks,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("只搜索浏览器书签");
                                        ui.end_row();

                                        ui.label("系统");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.system,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("只搜索系统与 Clibo 工具");
                                        ui.end_row();

                                        ui.label("Google");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.google,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("使用 Google 搜索网页");
                                        ui.end_row();

                                        ui.label("Bing");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.bing,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("使用 Bing 搜索网页");
                                        ui.end_row();

                                        ui.label("百度");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.baidu,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("使用百度搜索网页");
                                        ui.end_row();

                                        ui.label("网页");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.web,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("通用网页搜索（Bing）");
                                        ui.end_row();

                                        ui.label("计算");
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.settings.launcher_search_prefixes.calculator,
                                            )
                                            .desired_width(90.),
                                        );
                                        ui.label("直接计算表达式，例如 = 12*3");
                                        ui.end_row();
                                    });
                                        });
                                });
                            }
                            SettingsSection::Hotkeys => {
                                for (card, slots) in [
                                    (
                                        "全局快捷键",
                                        vec![
                                            (HotkeySlot::Show, "呼出面板", self.settings.hotkey.clone()),
                                            (
                                                HotkeySlot::Queue,
                                                "排列粘贴",
                                                self.settings.queue_hotkey.clone(),
                                            ),
                                            (
                                                HotkeySlot::Launcher,
                                                "启动器",
                                                self.settings.launcher_hotkey.clone(),
                                            ),
                                        ],
                                    ),
                                    (
                                        "面板内",
                                        vec![
                                            (
                                                HotkeySlot::Find,
                                                "查找",
                                                self.settings.find_hotkey.clone(),
                                            ),
                                            (
                                                HotkeySlot::Json,
                                                "JSON 工作页",
                                                self.settings.json_hotkey.clone(),
                                            ),
                                            (
                                                HotkeySlot::Timestamp,
                                                "时间戳转换",
                                                self.settings.timestamp_hotkey.clone(),
                                            ),
                                        ],
                                    ),
                                ] {
                                    settings_card(ui, &p, card, |ui| {
                                        for (index, (slot, label, value)) in
                                            slots.iter().enumerate()
                                        {
                                            if index > 0 {
                                                card_divider(ui, &p);
                                            }
                                            let active = self.hotkey_capture == Some(*slot);
                                            setting_row_sized(ui, &p, label, "", 280., |ui| {
                                                let text = if active {
                                                    pending_hotkey_label(ui.ctx())
                                                } else if value.is_empty() {
                                                    "未设置".into()
                                                } else {
                                                    pretty_hotkey(value)
                                                };
                                                let chip = ui.add(
                                                    egui::Button::new(
                                                        mono(text).size(11.5).color(if active {
                                                            p.accent
                                                        } else {
                                                            p.text
                                                        }),
                                                    )
                                                    .min_size(Vec2::new(150., 24.))
                                                    .fill(if active {
                                                        p.accent_soft
                                                    } else {
                                                        p.input
                                                    })
                                                    .stroke(egui::Stroke::new(
                                                        1.,
                                                        if active { p.accent } else { p.border },
                                                    ))
                                                    .corner_radius(CornerRadius::same(6)),
                                                );
                                                if chip.clicked() {
                                                    self.hotkey_capture =
                                                        if active { None } else { Some(*slot) };
                                                    self.capture_error.clear();
                                                }
                                                ui.add_space(14.);
                                                if ghost(ui, "重录").clicked() {
                                                    self.hotkey_capture = Some(*slot);
                                                    self.capture_error.clear();
                                                }
                                            });
                                        }
                                    });
                                }
                                if !self.capture_error.is_empty() {
                                    ui.label(
                                        RichText::new(&self.capture_error)
                                            .size(11.)
                                            .color(p.error),
                                    );
                                } else if !self.hotkey_error.is_empty() {
                                    ui.label(
                                        RichText::new(&self.hotkey_error)
                                            .size(11.)
                                            .color(p.error),
                                    );
                                } else if self.hotkey_capture.is_some() {
                                    ui.label(
                                        RichText::new("按下新组合键录入；Esc 取消")
                                            .size(11.)
                                            .color(p.text_dim),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new("点击组合键或「重录」后直接按键，无需手填")
                                            .size(11.)
                                            .color(p.text_dim),
                                    );
                                }
                            }
                            SettingsSection::Data => {
                                settings_card(ui, &p, "备份", |ui| {
                                    setting_row(
                                        ui,
                                        &p,
                                        "立即备份历史",
                                        "在数据目录生成带时间戳的备份文件",
                                        |ui| {
                                            if pill(ui, &p, "立即备份", false).clicked() {
                                                self.backup_history();
                                            }
                                        },
                                    );
                                    card_divider(ui, &p);
                                    setting_row(
                                        ui,
                                        &p,
                                        "恢复最近备份",
                                        "用最近一次手动备份替换当前历史，恢复前会自动再备份一次",
                                        |ui| {
                                            if pill(ui, &p, "恢复…", false).clicked() {
                                                self.confirm = Some("__restore__".into());
                                            }
                                        },
                                    );
                                });
                                settings_card(ui, &p, "清理", |ui| {
                                    setting_row(
                                        ui,
                                        &p,
                                        "清理未收藏历史",
                                        "删除所有未收藏的历史条目，收藏与分组保留",
                                        |ui| {
                                            if danger_pill(ui, &p, "清理…").clicked() {
                                                self.confirm = Some("__clear__".into());
                                            }
                                        },
                                    );
                                });
                            }
                        }
                                            });
                                        });
                                let footer_top = ui.cursor().top();
                                ui.painter().hline(
                                    ui.available_rect_before_wrap().x_range(),
                                    footer_top,
                                    egui::Stroke::new(1., p.hairline),
                                );
                                egui::Frame::default()
                                    .inner_margin(egui::Margin::symmetric(22, 9))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(mono("●").size(9.).color(p.accent));
                                            ui.label(
                                                RichText::new("更改在保存后生效")
                                                    .size(10.5)
                                                    .color(p.text_dim),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if primary(ui, &p, "保存设置").clicked() {
                                                        self.save_settings();
                                                    }
                                                },
                                            );
                                        });
                                    });
                            });
                        });
                });
            if !open {
                self.settings_open = false;
                self.hotkey_capture = None;
            }
        }
        if let Some(id) = self.confirm.clone() {
            let restoring = id == "__restore__";
            egui::Window::new(if restoring {
                "确认恢复"
            } else {
                "确认删除"
            })
            .collapsible(false)
            .resizable(false)
            .min_width(340.)
            .show(ctx, |ui| {
                let p = palette(self.dark, self.theme);
                ui.label(if restoring {
                    "使用最近一次手动备份替换当前历史和分组？恢复前会自动创建当前历史的安全备份。"
                } else if id == "__clear__" {
                    "删除所有未收藏历史？当前排列中的对应记录也会移除。"
                } else {
                    "删除这条历史及其排列引用？"
                });
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    if ghost(ui, "取消").clicked() {
                        self.confirm = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let on_error = if self.dark {
                            Color32::from_rgb(0x2A, 0x0E, 0x0B)
                        } else {
                            Color32::WHITE
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(if restoring {
                                        "确认恢复"
                                    } else {
                                        "确认删除"
                                    })
                                    .size(12.)
                                    .strong()
                                    .color(on_error),
                                )
                                .fill(p.error)
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(CornerRadius::same(11))
                                .min_size(Vec2::new(0., 22.)),
                            )
                            .clicked()
                        {
                            if restoring {
                                self.restore_history();
                            } else {
                                self.operation(|s| {
                                    if id == "__clear__" {
                                        s.clear(false)
                                    } else {
                                        s.delete(&id)
                                    }
                                });
                                self.preview_id.clear();
                            }
                            self.confirm = None;
                        }
                    });
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_hotkey_defaults_and_conflicts() {
        let mut settings = Settings::default();
        let keys = hotkey_specs(&settings).unwrap();
        assert_eq!(keys[2].1, HotKey::new(Some(HotkeyMods::ALT), Code::KeyS));
        settings.launcher_hotkey = settings.hotkey.clone();
        assert!(hotkey_specs(&settings).is_err());
        settings.launcher_hotkey = "invalid hotkey".into();
        assert!(hotkey_specs(&settings).is_err());
    }

    #[test]
    fn captured_hotkey_strings_roundtrip() {
        // 录入写入的字符串必须能被 register() 重新解析，否则保存后无法注册。
        let combos = [
            (HotkeyMods::CONTROL | HotkeyMods::ALT, Code::KeyV),
            (HotkeyMods::CONTROL | HotkeyMods::SHIFT, Code::KeyV),
            (HotkeyMods::CONTROL | HotkeyMods::SHIFT, Code::F11),
            (HotkeyMods::ALT, Code::Digit5),
            (HotkeyMods::ALT, Code::Numpad7),
            (HotkeyMods::SUPER, Code::ArrowDown),
            (
                HotkeyMods::CONTROL | HotkeyMods::ALT | HotkeyMods::SHIFT,
                Code::Backquote,
            ),
        ];
        for (mods, code) in combos {
            let hotkey = HotKey::new(Some(mods), code);
            assert_eq!(HotKey::from_str(&hotkey.to_string()).unwrap(), hotkey);
        }
    }

    #[test]
    fn find_shortcut_uses_saved_binding_and_consumes_only_matching_events() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                shift: true,
                ..Default::default()
            },
        });
        let _ = ctx.run(input, |ctx| {
            assert!(!consume_local_shortcut(ctx, "Ctrl+F"));
            assert!(consume_local_shortcut(ctx, "Ctrl+Shift+F"));
            assert!(!consume_local_shortcut(ctx, "Ctrl+Shift+F"));
        });
    }

    #[test]
    fn default_show_hotkey_is_ctrl_shift_v() {
        // 默认呼出快捷键；必须能解析注册并正确显示。
        let default = Settings::default().hotkey;
        assert_eq!(
            HotKey::from_str(&default).unwrap(),
            HotKey::new(Some(HotkeyMods::CONTROL | HotkeyMods::SHIFT), Code::KeyV)
        );
        assert_eq!(pretty_hotkey(&default), "Ctrl + Shift + V");
    }

    #[test]
    #[cfg(windows)]
    fn vk_table_covers_swallowed_keys() {
        // 被 egui-winit 吞掉的 V/C/X 必须能被轮询表识别，默认键位 V 也在列。
        for code in [Code::KeyV, Code::KeyC, Code::KeyX, Code::F24, Code::Numpad0] {
            assert!(HOTKEY_VKS.iter().any(|&(_, c)| c == code), "{code:?} 缺失");
        }
        // 失焦时不轮询，避免录入其他应用里的输入。
        assert!(poll_physical_hotkey(false).is_none());
    }

    #[test]
    fn pretty_hotkey_labels() {
        assert_eq!(pretty_hotkey("control+alt+KeyV"), "Ctrl + Alt + V");
        // 旧版手填的字符串也能正常显示
        assert_eq!(pretty_hotkey("Ctrl+Alt+V"), "Ctrl + Alt + V");
        assert_eq!(pretty_hotkey("shift+F12"), "Shift + F12");
        assert_eq!(pretty_hotkey("alt+ArrowDown"), "Alt + ↓");
        assert_eq!(
            pretty_hotkey("super+KeyV"),
            if cfg!(target_os = "macos") {
                "Cmd + V".to_string()
            } else {
                "Win + V".to_string()
            }
        );
    }

    #[test]
    fn egui_keys_map_to_codes() {
        assert_eq!(key_code(egui::Key::V), Some(Code::KeyV));
        assert_eq!(key_code(egui::Key::Num5), Some(Code::Digit5));
        assert_eq!(key_code(egui::Key::F35), Some(Code::F35));
        assert_eq!(key_code(egui::Key::Backtick), Some(Code::Backquote));
        // Shift 变体与虚拟键不支持录入
        assert_eq!(key_code(egui::Key::Colon), None);
        assert_eq!(key_code(egui::Key::Copy), None);
    }
}
