//! 菜单项，参照 macOS 26 的菜单：左侧图标、右侧快捷键或箭头，悬停时整行填充强调色，
//! 分隔线左右内缩。菜单底板（底色、描边、阴影）在 `style::install` 里设置。
//!
//! 面板很窄，子菜单在旁边放不下，所以不用 egui 的子菜单：带箭头的菜单项点击后在菜单内翻页。

use std::sync::Arc;

use egui::{
    Color32, FontId, Galley, Rect, Response, Sense, Stroke, Ui, Vec2, WidgetInfo, WidgetType,
};

use super::icons::{self, Icon};
use super::style::Palette;
use crate::keys;

/// 菜单项高度与标准控件一致；高亮是胶囊形，与菜单外框同心（`style::MENU_RADIUS`）。
const HEIGHT: f32 = 24.0;
/// 内容到高亮边缘的留白。
const PAD: f32 = 10.0;
const ICON: f32 = 14.0;
const ICON_GAP: f32 = 8.0;
/// 文字与右侧快捷键之间至少留出的距离。
const TRAILING_GAP: f32 = 28.0;
const CHEVRON: f32 = 10.0;
/// ↩ ⇥ 在 SF 中没有字形，改为矢量图标绘制（与 `panel::keycaps` 相同）。
const KEY_ICON: f32 = 11.0;

/// 菜单内容的公共设置：菜单项紧挨着排列（间隔由高亮的圆角体现），并限制最小宽度。
pub fn begin(ui: &mut Ui, min_width: f32) {
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.set_min_width(min_width);
}

pub struct Item<'a> {
    icon: Option<Icon>,
    label: String,
    shortcut: &'a [&'a str],
    danger: bool,
    chevron: bool,
}

impl<'a> Item<'a> {
    /// 没有图标时仍空出图标列，文字与其它菜单项对齐。
    pub fn new(icon: impl Into<Option<Icon>>, label: impl Into<String>) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            shortcut: &[],
            danger: false,
            chevron: false,
        }
    }

    /// 右侧显示的快捷键，写法同 [`keys::shortcut`]。
    pub fn shortcut(mut self, keys: &'a [&'a str]) -> Self {
        self.shortcut = keys;
        self
    }

    /// 右侧显示箭头：点击后进入下一页。
    pub fn chevron(mut self) -> Self {
        self.chevron = true;
        self
    }

    /// 删除等破坏性操作：红色文字，悬停时红底。
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn ui(self, ui: &mut Ui, palette: &Palette) -> Response {
        let info = WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &self.label);
        let label = ui.painter().layout_no_wrap(
            self.label,
            FontId::proportional(13.0),
            Color32::PLACEHOLDER,
        );
        let keys = shortcut_glyphs(ui, self.shortcut);
        let trailing = if self.chevron {
            CHEVRON
        } else {
            keys.iter().map(Glyph::width).sum::<f32>()
        };
        let mut width = PAD + ICON + ICON_GAP + label.size().x + PAD;
        if trailing > 0.0 {
            width += TRAILING_GAP + trailing;
        }
        if ui.layout().horizontal_justify() {
            width = width.max(ui.available_width());
        }
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());
        response.widget_info(|| info.clone());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let highlighted = response.hovered() || response.has_focus();
        let tint = if self.danger {
            palette.danger
        } else {
            palette.accent
        };
        let (text, weak) = if highlighted {
            ui.painter().rect_filled(rect, HEIGHT / 2.0, tint);
            (Color32::WHITE, Color32::from_white_alpha(200))
        } else if self.danger {
            (palette.danger, palette.weak)
        } else {
            (palette.text, palette.weak)
        };

        let painter = ui.painter();
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + PAD + ICON / 2.0, rect.center().y),
            Vec2::splat(ICON),
        );
        if let Some(icon) = self.icon {
            icons::paint(painter, icon_rect, icon, text);
        }
        let label_pos = egui::pos2(
            icon_rect.right() + ICON_GAP,
            rect.center().y - label.size().y / 2.0,
        );
        painter.galley(label_pos, label, text);

        let mut x = rect.right() - PAD;
        if self.chevron {
            let chevron = Rect::from_center_size(
                egui::pos2(x - CHEVRON / 2.0, rect.center().y),
                Vec2::splat(CHEVRON),
            );
            icons::paint(painter, chevron, Icon::ChevronRight, weak);
        }
        // 快捷键靠右对齐，从最后一个键往左画。
        for glyph in keys.into_iter().rev() {
            x -= glyph.width();
            match glyph {
                Glyph::Text(galley) => {
                    let pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
                    painter.galley(pos, galley, weak);
                }
                Glyph::Icon(icon) => {
                    let key = Rect::from_center_size(
                        egui::pos2(x + KEY_ICON / 2.0, rect.center().y),
                        Vec2::splat(KEY_ICON),
                    );
                    icons::paint(painter, key, icon, weak);
                }
            }
        }
        response
    }
}

/// 菜单里的小标题，与菜单项文字左对齐。
pub fn heading(ui: &mut Ui, palette: &Palette, text: &str) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(12.0), palette.weak);
    let size = Vec2::new(PAD * 2.0 + galley.size().x, HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let pos = egui::pos2(rect.left() + PAD, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, palette.weak);
}

/// 左右内缩的细分隔线。
pub fn separator(ui: &mut Ui, palette: &Palette) {
    // 宽度为 0：不撑宽菜单，线条按菜单内容区的宽度画。
    let (rect, _) = ui.allocate_exact_size(Vec2::new(0.0, 9.0), Sense::hover());
    let x = ui.max_rect().x_range();
    let y = ui.painter().round_to_pixel_center(rect.center().y);
    ui.painter().hline(
        x.min + PAD..=x.max - PAD,
        y,
        Stroke::new(1.0, palette.border),
    );
}

enum Glyph {
    Text(Arc<Galley>),
    Icon(Icon),
}

impl Glyph {
    fn width(&self) -> f32 {
        match self {
            Glyph::Text(galley) => galley.size().x,
            Glyph::Icon(_) => KEY_ICON,
        }
    }
}

/// macOS 上逐个排版按键符号（↩ ⇥ 用图标），其它平台整体排成 `Ctrl+Enter`。
fn shortcut_glyphs(ui: &Ui, parts: &[&str]) -> Vec<Glyph> {
    let text = |text: String| {
        Glyph::Text(ui.painter().layout_no_wrap(
            text,
            FontId::proportional(12.0),
            Color32::PLACEHOLDER,
        ))
    };
    if parts.is_empty() {
        Vec::new()
    } else if cfg!(target_os = "macos") {
        parts
            .iter()
            .map(|&key| match key {
                "↩" => Glyph::Icon(Icon::Return),
                "⇥" => Glyph::Icon(Icon::Tab),
                key => text(key.to_string()),
            })
            .collect()
    } else {
        vec![text(keys::shortcut(parts))]
    }
}
