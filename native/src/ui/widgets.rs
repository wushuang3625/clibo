//! 自绘小组件：精确的矢量几何与配色，替代 egui 默认控件样式。
use super::theme::Palette;
use eframe::egui::{self, Color32, CornerRadius, RichText, Vec2};

/// Monospace helper for metadata, counters and keyboard hints.
pub(crate) fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text).family(egui::FontFamily::Monospace)
}
pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}
/// Small caps section label with a trailing hairline rule.
pub(crate) fn section(ui: &mut egui::Ui, p: &Palette, label: &str) {
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
pub(crate) fn pill(ui: &mut egui::Ui, p: &Palette, label: &str, active: bool) -> egui::Response {
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
pub(crate) fn ghost(ui: &mut egui::Ui, label: impl Into<String>) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).size(12.)).frame(false))
}
/// Font-independent line icons with the same geometry and feedback as filter pills.
pub(crate) fn icon_button(
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
pub(crate) fn primary(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(12.).strong().color(p.on_accent))
            .fill(p.accent)
            .stroke(egui::Stroke::NONE)
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(0., 22.)),
    )
}
/// Danger action using the same pill geometry as the rest of the UI.
pub(crate) fn danger_pill(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(mono(label).size(11.).color(p.error))
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1., p.error))
            .corner_radius(CornerRadius::same(11))
            .min_size(Vec2::new(0., 22.)),
    )
}
/// Outlined mono tag for the entry kind.
pub(crate) fn kind_tag(ui: &mut egui::Ui, p: &Palette, label: &str) {
    egui::Frame::default()
        .stroke(egui::Stroke::new(1., p.border))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(mono(label).size(10.).color(p.accent));
        });
}
/// Borderless icon button for the search row; only hover reveals the chip.
pub(crate) fn ghost_icon(
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
pub(crate) fn kbd(ui: &mut egui::Ui, p: &Palette, label: &str) {
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
pub(crate) fn toggle(ui: &mut egui::Ui, p: &Palette, value: &mut bool, hint: &str) -> bool {
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
pub(crate) fn stepper<T>(ui: &mut egui::Ui, p: &Palette, value: &mut T, min: T, max: T, step: T)
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
pub(crate) fn settings_card(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
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
pub(crate) fn card_divider(ui: &mut egui::Ui, p: &Palette) {
    ui.painter().hline(
        ui.available_rect_before_wrap().x_range(),
        ui.cursor().top(),
        egui::Stroke::new(1., p.hairline),
    );
}
/// Settings row: label + description on the left, control on the right.
pub(crate) fn setting_row(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    desc: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    setting_row_sized(ui, p, title, desc, 170., control)
}
/// Same row with a custom reserved control width for wider controls (e.g. hotkey chips).
pub(crate) fn setting_row_sized(
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
pub(crate) fn settings_nav_item(
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Clipboard,
    Launcher,
    Hotkeys,
    Data,
}
impl SettingsSection {
    pub(crate) const ALL: [SettingsSection; 5] = [
        SettingsSection::General,
        SettingsSection::Clipboard,
        SettingsSection::Launcher,
        SettingsSection::Hotkeys,
        SettingsSection::Data,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            SettingsSection::General => "通用",
            SettingsSection::Clipboard => "剪贴板",
            SettingsSection::Launcher => "启动器",
            SettingsSection::Hotkeys => "快捷键",
            SettingsSection::Data => "数据与备份",
        }
    }
    /// Keywords matched by the in-settings search box.
    pub(crate) fn keywords(self) -> &'static str {
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
    pub(crate) fn matches(self, query: &str) -> bool {
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
