//! 外观主题：琥珀 / 蓝灰双主题的调色板、egui 视觉配置与字体安装。
use eframe::egui::{self, Color32, CornerRadius, Vec2};

/// 圆角主窗口统一使用的圆角半径。
pub(crate) const APP_CORNER_RADIUS: u8 = 14;

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) panel: Color32,
    pub(crate) card: Color32,
    pub(crate) card_hover: Color32,
    pub(crate) card_selected: Color32,
    pub(crate) border: Color32,
    pub(crate) hairline: Color32,
    pub(crate) text: Color32,
    pub(crate) text_dim: Color32,
    pub(crate) accent: Color32,
    pub(crate) accent_soft: Color32,
    pub(crate) on_accent: Color32,
    pub(crate) star: Color32,
    pub(crate) error: Color32,
    pub(crate) input: Color32,
}
/// 双主题：琥珀 = "精密仪器"（暖黑曜石 + 琥珀信号色，浅色为暖纸面 + 墨色）；
/// 蓝灰 = 启动器原配色（深空蓝黑 + 长春花蓝信号色，浅色为冷纸面）。
pub(crate) fn palette(dark: bool, theme: Theme) -> Palette {
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
pub(crate) enum Theme {
    #[default]
    Amber,
    Blue,
}
impl Theme {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Theme::Amber => "琥珀",
            Theme::Blue => "蓝灰",
        }
    }
}

pub(crate) fn apply_theme(ctx: &egui::Context, dark: bool, theme: Theme) {
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
pub(crate) fn install_fonts(ctx: &egui::Context) {
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
