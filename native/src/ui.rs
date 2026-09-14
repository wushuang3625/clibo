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
mod timestamp_ui;

const ROW_HEIGHT: f32 = 64.;
const ROW_STEP: f32 = ROW_HEIGHT + 8.;
/// Bounded row-thumbnail texture cache (24 × ~320×240 ≈ 7 MB of GPU memory).
const THUMB_CACHE: usize = 24;
/// Maximum row thumbnail jobs queued per frame; decoding happens off the UI thread.
const THUMB_BUDGET: u8 = 4;

enum Event {
    Open(bool, usize),
    OpenJson,
    OpenTimestamp,
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
    calculator: crate::calculator::Calculator,
    json: json_ui::JsonPage,
    json_window: json_ui::WindowTransition,
    timestamp: timestamp_ui::TimestampPage,
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
    #[cfg(windows)]
    autostart: Result<bool, String>,
    settings: Settings,
    excluded: String,
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
    ime_composing: bool,
    smoke: Option<std::time::Instant>,
    screenshot_requested: bool,
    dark: bool,
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

fn register(manager: &GlobalHotKeyManager, settings: &Settings) -> Result<Vec<HotKey>, String> {
    let main =
        HotKey::from_str(&settings.hotkey).map_err(|e| format!("呼出快捷键格式错误：{e}"))?;
    let queue =
        HotKey::from_str(&settings.queue_hotkey).map_err(|e| format!("队列快捷键格式错误：{e}"))?;
    if main == queue {
        return Err("两个快捷键不能相同".into());
    }
    manager
        .register(main)
        .map_err(|e| format!("呼出快捷键冲突：{e}"))?;
    if let Err(e) = manager.register(queue) {
        let _ = manager.unregister(main);
        return Err(format!("队列快捷键冲突：{e}"));
    }
    Ok(vec![main, queue])
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
            .with_title("Clibo · Native")
            .with_decorations(false)
            .with_inner_size(
                if std::env::args().any(|a| a == "--smoke")
                    && std::env::args().any(|a| a == "--small")
                {
                    [620., 440.]
                } else {
                    [780., 580.]
                },
            )
            .with_min_inner_size([620., 440.])
            .with_always_on_top(),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Clibo Native",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx, ui_state.dark);
            let backend = Backend::new(store, cc.egui_ctx.clone());
            let (image_tx, image_rx) =
                spawn_image_worker(history_path.clone(), cc.egui_ctx.clone());
            let settings = backend.store.lock().unwrap().settings.clone();
            let hotkeys = GlobalHotKeyManager::new()?;
            let (registered, hotkey_error) = match register(&hotkeys, &settings) {
                Ok(keys) => (keys, String::new()),
                Err(e) => (vec![], e),
            };
            let (tx, rx) = mpsc::channel();
            let open_tx = tx.clone();
            let open_backend = backend.clone();
            let open_ctx = cc.egui_ctx.clone();
            instance.listen(move || {
                dispatch(&open_tx, Event::Open(false, 0), &open_backend, &open_ctx);
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
                    let queue = HotKey::from_str(&settings.queue_hotkey)
                        .is_ok_and(|key| key.id() == event.id);
                    dispatch(
                        &hot_tx,
                        Event::Open(queue, platform::foreground()),
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
            let quit = MenuItem::new("退出", true, None);
            menu.append_items(&[&open, &queue, &json, &timestamp, &quit])?;
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
                } else if event.id == json_id {
                    dispatch(&tx, Event::OpenJson, &menu_backend, &ctx);
                } else if event.id == timestamp_id {
                    dispatch(&tx, Event::OpenTimestamp, &menu_backend, &ctx);
                } else if event.id == open_id || event.id == queue_id {
                    dispatch(
                        &tx,
                        Event::Open(event.id == queue_id, 0),
                        &menu_backend,
                        &ctx,
                    );
                }
            }));
            let icon_image =
                image::load_from_memory(include_bytes!("../icons/icon.png"))?.into_rgba8();
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
                settings,
                query: String::new(),
                json: json_ui::JsonPage::initial(),
                json_window: json_ui::WindowTransition::default(),
                timestamp: timestamp_ui::TimestampPage::default(),
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
                preview_open: true,
                ime_composing: false,
                smoke: std::env::args()
                    .any(|a| a == "--smoke")
                    .then(std::time::Instant::now),
                screenshot_requested: false,
                dark: ui_state.dark,
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
/// "精密仪器" 双主题：暖黑曜石 + 琥珀信号色，浅色为暖纸面 + 墨色。
fn palette(dark: bool) -> Palette {
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
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct UiState {
    #[serde(default)]
    dark: bool,
}
impl UiState {
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
fn apply_theme(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
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
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(6);
    let shadow_alpha = if dark { 90 } else { 40 };
    let shadow = egui::Shadow {
        offset: [0, 3],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    visuals.window_shadow = shadow;
    visuals.popup_shadow = shadow;
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
    fn save_settings(&mut self) {
        let mut next = self.settings.clone();
        next.excluded_apps = self
            .excluded
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if let Err(e) = next.validate() {
            self.backend.report(Err(e));
            return;
        }
        let old = self.backend.store.lock().unwrap().settings.clone();
        for key in &self.registered {
            let _ = self.hotkeys.unregister(*key);
        }
        self.registered.clear();
        match register(&self.hotkeys, &next) {
            Ok(keys) => self.registered = keys,
            Err(e) => {
                self.registered = register(&self.hotkeys, &old).unwrap_or_default();
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
            self.registered = register(&self.hotkeys, &old).unwrap_or_default();
        } else {
            self.settings = next;
            self.hotkey_error.clear();
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
        }
        self.hotkey_capture = None;
        self.capture_error.clear();
    }
    /// 48×36 row thumbnail. The UI only queues bounded work and paints a
    /// placeholder; SQLite reads, decryption, resizing and pixel decoding all run
    /// on the image worker.
    fn row_thumbnail(&mut self, ui: &mut egui::Ui, item: &ClipView) {
        let p = palette(self.dark);
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
            if started.elapsed() > std::time::Duration::from_secs(2) && !self.screenshot_requested {
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
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::OpenJson | Event::OpenTimestamp => {
                    self.calculator.leave();
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
                Event::Quit => {
                    self._tray = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                Event::Open(queue, target) => {
                    if platform::valid_target(target, self.owner) {
                        self.target = target;
                    }
                    self.backend.visible.store(true, Ordering::Relaxed);
                    if queue {
                        self.timestamp.active = false;
                        self.json.active = false;
                        self.calculator.leave();
                    }
                    if !self.calculator.active {
                        self.query.clear();
                        self.group.clear();
                        self.selected.clear();
                        self.filter = if queue { "queue" } else { "all" }.into();
                    }
                    self.calculator.focus = true;
                    self.settings_open = false;
                    self.confirm = None;
                    self.dirty = true;
                    self.focus_search = true;
                    self.was_focused = false;
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
        resize_border(ctx);
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if self.was_focused
            && !focused
            && !self.pinned
            && !self.calculator.active
            && !self.json.active
            && !self.timestamp.active
            && !self.settings_open
            && self.confirm.is_none()
        {
            self.hide(ctx);
        }
        self.was_focused = focused;
        self.json_window.update(ctx, self.json.active, self.owner);
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
        let mut navigation_active = false;
        egui::TopBottomPanel::top("brand-strip")
            .exact_height(2.)
            .frame(egui::Frame::NONE.fill(palette(self.dark).accent))
            .show(ctx, |_ui| {});
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::NONE
                    .fill(palette(self.dark).panel)
                    .inner_margin(egui::Margin::symmetric(12, 6)),
            )
            .show(ctx, |ui| {
            let p = palette(self.dark);
            ui.horizontal(|ui| {
                let (mark, _) = ui.allocate_exact_size(Vec2::new(12., 12.), egui::Sense::hover());
                ui.painter().rect_filled(mark, CornerRadius::same(3), p.accent);
                ui.painter()
                    .rect_filled(mark.shrink(3.5), CornerRadius::same(1), p.panel);
                ui.label(mono("CLIBO").size(15.).strong().color(p.text));
                ui.label(RichText::new("本地剪贴板").size(11.).color(p.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("JSON").on_hover_text("JSON 格式化与字段搜索").clicked() {
                        self.json.active = true;
                    }
                    if ui.button("时间戳").on_hover_text("时间戳与日期时间转换").clicked() {
                        self.timestamp.active = true;
                    }
                    if icon_button(ui, &p, "hide", false, "收起窗口 · Esc").clicked() {
                        self.hide(ctx);
                    }
                    if icon_button(ui, &p, "settings", self.settings_open, "偏好设置").clicked() {
                        #[cfg(windows)]
                        { self.autostart = crate::autostart::enabled(); }
                        self.settings = self.backend.store.lock().unwrap().settings.clone();
                        self.excluded = self.settings.excluded_apps.join("\n");
                        self.settings_open = true;
                        self.hotkey_capture = None;
                        self.capture_error.clear();
                    }
                    if icon_button(ui, &p, if self.dark { "sun" } else { "moon" }, false, if self.dark { "切换为浅色外观" } else { "切换为深色外观" }).clicked() {
                        self.dark = !self.dark;
                        apply_theme(ctx, self.dark);
                        if let Ok(dir) = backend::data_dir() {
                            UiState { dark: self.dark }.save(&dir);
                        }
                    }
                    if icon_button(ui, &p, "pin", self.pinned, if self.pinned { "取消固定 · 点击窗口外自动收起" } else { "固定窗口 · 点击窗口外保持显示" }).clicked() {
                        self.pinned = !self.pinned;
                    }
                    let enabled = self.backend.store.lock().unwrap().settings.enabled;
                    let record = if enabled { "● 记录中" } else { "○ 已暂停" };
                    if pill(ui, &p, record, enabled).clicked() {
                        self.operation(|s| {
                            let mut c = s.settings.clone();
                            c.enabled = !c.enabled;
                            s.save_settings(c)
                        });
                    }
                    let drag = ui.allocate_response(ui.available_size(), egui::Sense::drag());
                    if drag.drag_started() { ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); }
                });
            });
            ui.add_space(4.);
            let search = ui
                .horizontal(|ui| {
                    ui.label(mono("›").size(14.).color(p.accent));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("搜索内容、来源、分组或备注，输入 = 计算…")
                            .desired_width(f32::INFINITY)
                            .char_limit(4096),
                    )
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
            }
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            navigation_active = search.has_focus() || !ctx.wants_keyboard_input();
            ui.add_space(4.);
            let previous_row_height = ui.spacing().interact_size.y;
            ui.spacing_mut().interact_size.y = 24.;
            ui.horizontal(|ui| {
                let organizer = self.backend.store.lock().unwrap().organizer_view();
                {
                    // Categories and groups share one layout and baseline.
                    // Overflow scrolls together while the right actions stay fixed.
                    let group_width = (ui.available_width() - 180.).max(48.);
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
                    if pill(ui, &p, &format!("排列粘贴 · {count}"), self.filter == "queue").clicked() {
                        self.filter = if self.filter == "queue" { "all" } else { "queue" }.into();
                        self.query.clear();
                        self.group.clear();
                        self.dirty = true;
                    }
                    if pill(ui, &p, "预览", self.preview_open).clicked() {
                        self.preview_open = !self.preview_open;
                    }
                });
            });
            ui.spacing_mut().interact_size.y = previous_row_height;
            if !self.hotkey_error.is_empty() {
                ui.colored_label(p.error, mono(&self.hotkey_error).size(11.));
            }
            if self.filter == "queue" {
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
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(palette(self.dark).panel)
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ctx, |ui| {
                let p = palette(self.dark);
                ui.horizontal(|ui| {
                    ui.label(
                        mono(format!("{:>4} 条", self.items.len()))
                            .size(11.)
                            .color(p.accent),
                    );
                    ui.label(
                        mono(self.backend.status.lock().unwrap().clone())
                            .size(11.)
                            .color(p.text_dim),
                    );
                    if busy {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            mono("↑↓ 选择 · ⏎ 粘贴 · ⇧⏎ 复制 · Esc 收起")
                                .size(11.)
                                .color(p.text_dim),
                        );
                    });
                });
            });
        if self.preview_open {
            self.preview(ctx);
            if self.smoke.is_some()
                && std::env::args().any(|a| a == "--enlarge")
                && self.texture.is_some()
                && self.enlarged_image.is_none()
            {
                self.enlarge_image(ctx);
            }
            egui::SidePanel::right("preview")
                .default_width(280.)
                .min_width(220.)
                .frame(
                    egui::Frame::NONE
                        .fill(palette(self.dark).card)
                        .inner_margin(egui::Margin::symmetric(12, 8)),
                )
                .show(ctx, |ui| {
                    let p = palette(self.dark);
                    ui.add_enabled_ui(!busy && !modal, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(mono("预览").size(13.).strong().color(p.text));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if primary(ui, &p, "粘贴").clicked() {
                                        self.apply(true, false, false);
                                    }
                                    if pill(ui, &p, "仅复制", false).clicked() {
                                        self.apply(false, false, false);
                                    }
                                },
                            );
                        });
                        egui::ScrollArea::vertical()
                            .id_salt("preview-body")
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .auto_shrink([false, false])
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                ui.add_space(4.);
                                if let Some(texture) = &self.texture {
                                    if ui
                                        .add(
                                            egui::Image::new(texture)
                                                .max_size(Vec2::new(ui.available_width(), 200.))
                                                .sense(egui::Sense::click())
                                                .corner_radius(CornerRadius::same(4)),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::ZoomIn)
                                        .on_hover_text("点击放大图片")
                                        .clicked()
                                    {
                                        self.enlarge_image(ctx);
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
                                } else {
                                    egui::Frame::default()
                                        .fill(p.input)
                                        .stroke(egui::Stroke::new(1., p.hairline))
                                        .corner_radius(CornerRadius::same(6))
                                        .inner_margin(egui::Margin::symmetric(8, 6))
                                        .show(ui, |ui| {
                                            egui::ScrollArea::vertical()
                                                .id_salt("text-preview")
                                                .max_height((ui.available_height() * 0.46).max(80.))
                                                .show(ui, |ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&self.preview_text)
                                                                .size(12.5),
                                                        )
                                                        .wrap()
                                                        .selectable(true),
                                                    );
                                                });
                                        });
                                }
                                if !self.preview_is_image
                                    && self.texture.is_none()
                                    && !self.selected.is_empty()
                                {
                                    ui.add_space(6.);
                                    section(ui, &p, "文字处理");
                                    if ui.button("在 JSON 工作页中打开 →").clicked() {
                                        let source = self
                                            .backend
                                            .store
                                            .lock()
                                            .unwrap()
                                            .secret(&self.selected);
                                        match source {
                                            Ok(secret) => {
                                                self.json.source = secret.text.unwrap_or_default();
                                                self.json.parse();
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
                                        match crate::text_tools::transform(&source, &self.transform)
                                        {
                                            Ok(text) => {
                                                self.preview_text =
                                                    text.chars().take(32_000).collect()
                                            }
                                            Err(e) => {
                                                self.transform = old;
                                                self.backend.report(Err(e));
                                            }
                                        }
                                    }
                                }
                                ui.add_space(6.);
                                section(ui, &p, "备注与分组");
                                ui.add_space(2.);
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.note)
                                        .hint_text("备注，不会随内容粘贴")
                                        .desired_rows(2)
                                        .char_limit(1000),
                                );
                                let groups =
                                    self.backend.store.lock().unwrap().organizer.groups.clone();
                                if !groups.is_empty() {
                                    ui.add_space(4.);
                                    ui.label(mono("分组").size(10.5).color(p.text_dim));
                                    egui::ScrollArea::vertical()
                                        .id_salt("memberships")
                                        .scroll_bar_visibility(
                                            egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                        )
                                        .max_height(72.)
                                        .show(ui, |ui| {
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
                                        });
                                }
                                ui.add_space(4.);
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
                                        .corner_radius(CornerRadius::same(11)),
                                    )
                                    .clicked()
                                {
                                    let id = self.selected.clone();
                                    let groups = self.memberships.clone();
                                    let note = self.note.clone();
                                    self.operation(|s| s.annotate(&id, &groups, &note));
                                }
                                ui.add_space(2.);
                                let pinned = self
                                    .items
                                    .iter()
                                    .find(|item| item.id == self.selected)
                                    .is_some_and(|item| item.pinned);
                                ui.horizontal(|ui| {
                                    if pill(
                                        ui,
                                        &p,
                                        if pinned { "取消收藏" } else { "收藏" },
                                        pinned,
                                    )
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
                    });
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(!busy && !modal, |ui| {
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
                                    .color(palette(self.dark).text_dim),
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
                                    .color(palette(self.dark).text_dim),
                                );
                            });
                        });
                    });
                    return;
                }
                if self.history_up_distance > 140. && self.history_offset > ROW_STEP * 3. {
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
                    if let Some(index) = self.items.iter().position(|e| e.id == self.selected) {
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
                let history_scroll = area.show_rows(ui, ROW_HEIGHT, items.len(), |ui, range| {
                    let p = palette(self.dark);
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
                                        mono(if queued { "−" } else { "+" }).color(p.accent),
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
                            let width = ui.available_width() - if in_queue { 64. } else { 0. };
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
                            ui.painter()
                                .add(egui::Shape::rect_filled(rect, corner, fill));
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
                                            ui.label(RichText::new("★").color(p.star));
                                        }
                                        let summary = item
                                            .preview
                                            .split_whitespace()
                                            .collect::<Vec<_>>()
                                            .join(" ");
                                        let mut title: String = summary.chars().take(75).collect();
                                        if summary.chars().count() > 75 {
                                            title.push('…');
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(title).strong().color(p.text),
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
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    mono(time_label(item.copied_at))
                                                        .size(10.5)
                                                        .color(p.text_dim),
                                                );
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
                                for (label, delta) in [("↑", -1isize), ("↓", 1)] {
                                    if ui.small_button(mono(label).color(p.text_dim)).clicked() {
                                        let id = item.id.clone();
                                        self.operation(|s| {
                                            let mut q = s.organizer.queue.clone();
                                            if let Some(i) = q.iter().position(|v| v == &id) {
                                                let j = i as isize + delta;
                                                if j >= 0 && (j as usize) < q.len() {
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
            let p = palette(self.dark);
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
                        .fill(palette(self.dark).panel)
                        .stroke(egui::Stroke::new(1., palette(self.dark).border))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(16)),
                )
                .resizable(false)
                .min_width(380.)
                .show(ctx, |ui| {
                    let p = palette(self.dark);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("偏好设置").size(16.).strong().color(p.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, &p, "close", false, "关闭设置").clicked() {
                                open = false;
                            }
                        });
                    });
                    ui.add_space(6.);
                    egui::ScrollArea::vertical()
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .max_height((ctx.content_rect().height() - 160.).max(100.))
                        .show(ui, |ui| {
                            #[cfg(windows)]
                            {
                                section(ui, &p, "启动");
                                match self.autostart.clone() {
                                    Ok(mut enabled) => {
                                        if ui.checkbox(&mut enabled, "开机自启（立即生效）").changed() {
                                            let result = crate::autostart::set_enabled(enabled);
                                            self.backend.report(result);
                                            self.autostart = crate::autostart::enabled();
                                        }
                                    }
                                    Err(error) => { ui.colored_label(p.error, error); }
                                }
                                ui.label(RichText::new("登录 Windows 后在托盘后台运行。移动程序后请重新开启。").size(11.).color(p.text_dim));
                                ui.add_space(8.);
                            }
                            section(ui, &p, "采集");
                            ui.add_space(2.);
                            ui.checkbox(&mut self.settings.capture_images, "记录图片");
                            ui.checkbox(&mut self.settings.detect_sensitive, "跳过疑似密钥");
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("历史上限").size(12.).color(p.text));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(mono("条").size(11.).color(p.text_dim));
                                        egui::Frame::default()
                                            .fill(p.input)
                                            .stroke(egui::Stroke::new(1., p.hairline))
                                            .corner_radius(CornerRadius::same(4))
                                            .inner_margin(egui::Margin::symmetric(8, 2))
                                            .show(ui, |ui| {
                                                ui.add_sized(
                                                    Vec2::new(58., 18.),
                                                    egui::DragValue::new(
                                                        &mut self.settings.max_items,
                                                    )
                                                    .range(100..=2000)
                                                    .speed(50),
                                                );
                                            });
                                    },
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("保留天数").size(12.).color(p.text));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(mono("天").size(11.).color(p.text_dim));
                                        egui::Frame::default()
                                            .fill(p.input)
                                            .stroke(egui::Stroke::new(1., p.hairline))
                                            .corner_radius(CornerRadius::same(4))
                                            .inner_margin(egui::Margin::symmetric(8, 2))
                                            .show(ui, |ui| {
                                                ui.add_sized(
                                                    Vec2::new(58., 18.),
                                                    egui::DragValue::new(
                                                        &mut self.settings.retention_days,
                                                    )
                                                    .range(1..=365)
                                                    .speed(1),
                                                );
                                            });
                                    },
                                );
                            });
                            ui.add_space(4.);
                            section(ui, &p, "快捷键");
                            ui.add_space(2.);
                            for (slot, label, value) in [
                                (HotkeySlot::Show, "呼出面板", self.settings.hotkey.clone()),
                                (HotkeySlot::Find, "查找（应用内）", self.settings.find_hotkey.clone()),
                                (
                                    HotkeySlot::Queue,
                                    "排列粘贴",
                                    self.settings.queue_hotkey.clone(),
                                ),
                            ] {
                                let active = self.hotkey_capture == Some(slot);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(label).size(12.).color(p.text));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let text = if active {
                                                pending_hotkey_label(ctx)
                                            } else if value.is_empty() {
                                                "未设置".into()
                                            } else {
                                                pretty_hotkey(&value)
                                            };
                                            if ui
                                                .add(
                                                    egui::Button::new(mono(text).size(12.).color(
                                                        if active { p.accent } else { p.text },
                                                    ))
                                                    .min_size(Vec2::new(150., 24.))
                                                    .fill(if active {
                                                        p.accent_soft
                                                    } else {
                                                        p.input
                                                    })
                                                    .stroke(egui::Stroke::new(
                                                        1.,
                                                        if active { p.accent } else { p.hairline },
                                                    ))
                                                    .corner_radius(CornerRadius::same(4)),
                                                )
                                                .clicked()
                                            {
                                                self.hotkey_capture =
                                                    if active { None } else { Some(slot) };
                                                self.capture_error.clear();
                                            }
                                        },
                                    );
                                });
                            }
                            if !self.capture_error.is_empty() {
                                ui.label(
                                    RichText::new(&self.capture_error).size(11.).color(p.error),
                                );
                            } else if !self.hotkey_error.is_empty() {
                                ui.label(
                                    RichText::new(&self.hotkey_error).size(11.).color(p.error),
                                );
                            } else if self.hotkey_capture.is_some() {
                                ui.label(
                                    RichText::new("按下新组合键录入；Esc 取消")
                                        .size(11.)
                                        .color(p.text_dim),
                                );
                            } else {
                                ui.label(
                                    RichText::new("点击右侧组合后直接按键，无需手填")
                                        .size(11.)
                                        .color(p.text_dim),
                                );
                            }
                            ui.add_space(4.);
                            section(ui, &p, "排除应用 · 每行一个进程名");
                            ui.add_space(2.);
                            ui.add(
                                egui::TextEdit::multiline(&mut self.excluded)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(4),
                            );
                            ui.add_space(4.);
                            section(ui, &p, "数据安全");
                            ui.add_space(2.);
                            if let Some(status) =
                                self.backend.store.lock().unwrap().recovery_status()
                            {
                                ui.label(RichText::new(status).size(11.).color(p.error));
                            }
                            ui.horizontal_wrapped(|ui| {
                                if pill(ui, &p, "立即备份历史", false).clicked() {
                                    self.backup_history();
                                }
                                if pill(ui, &p, "恢复最近备份…", false).clicked() {
                                    self.confirm = Some("__restore__".into());
                                    self.settings_open = false;
                                }
                            });
                            ui.label(
                                RichText::new(
                                    "备份保存在数据目录；恢复前会自动再保存当前历史，当前快捷键和采集设置不变。",
                                )
                                .size(10.5)
                                .color(p.text_dim),
                            );
                            ui.add_space(4.);
                            section(ui, &p, "分组管理");
                            ui.add_space(2.);
                            let groups =
                                self.backend.store.lock().unwrap().organizer.groups.clone();
                            for g in groups {
                                ui.horizontal(|ui| {
                                    ui.label(&g.name);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if danger_pill(ui, &p, "解散").clicked() {
                                                self.operation(|s| s.group(&g.id, "", true));
                                            }
                                        },
                                    );
                                });
                            }
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.group_name)
                                        .hint_text("新分组名称")
                                        .char_limit(40),
                                );
                                if pill(ui, &p, "创建", false).clicked() {
                                    let name = self.group_name.clone();
                                    self.operation(|s| s.group("", &name, false));
                                    self.group_name.clear();
                                }
                            });
                            ui.add_space(4.);
                            if danger_pill(ui, &p, "清理未收藏历史…").clicked() {
                                self.confirm = Some("__clear__".into());
                                self.settings_open = false;
                            }
                        });
                    ui.add_space(6.);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if primary(ui, &p, "保存设置").clicked() {
                            self.save_settings();
                        }
                        if pill(ui, &p, "关闭", false).clicked() {
                            open = false;
                        }
                    });
                });
            if !open {
                self.settings_open = false;
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
                let p = palette(self.dark);
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
