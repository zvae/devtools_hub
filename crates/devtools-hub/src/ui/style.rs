//! 字体、配色与格式化工具。

use devtools_clipboard::ContentKind;
use egui::{Color32, FontFamily, FontId, TextStyle, Theme};

use super::fonts;
use super::icons::Icon;

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts::definitions());
    for theme in [Theme::Light, Theme::Dark] {
        let palette = Palette::new(theme == Theme::Dark, false);
        ctx.style_mut_of(theme, |style| {
            // 字号与 macOS 系统界面接近：正文 13，小字 11。
            style.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(17.0, FontFamily::Name(fonts::SEMIBOLD.into())),
                ),
                (TextStyle::Body, FontId::proportional(13.0)),
                (TextStyle::Button, FontId::proportional(13.0)),
                (TextStyle::Monospace, FontId::monospace(12.5)),
                (TextStyle::Small, FontId::proportional(11.0)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(6.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
            style.interaction.selectable_labels = true;
            let visuals = &mut style.visuals;
            visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
            visuals.selection.stroke.color = palette.accent;
            visuals.hyperlink_color = palette.accent;
            visuals.window_corner_radius = 10.into();
            visuals.menu_corner_radius = 8.into();
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
                &mut visuals.widgets.noninteractive,
            ] {
                widget.corner_radius = 6.into();
            }
        });
    }
}

/// 面板配色。开启毛玻璃时底色半透明，让背后的模糊效果透出来。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_stroke: Color32,
    pub field: Color32,
    /// 开关关闭时的轨道颜色。
    pub track: Color32,
    pub warn: Color32,
    pub ok: Color32,
}

impl Palette {
    pub fn new(dark: bool, translucent: bool) -> Self {
        let accent = Color32::from_rgb(0x30, 0x7b, 0xf6);
        if dark {
            Self {
                accent,
                background: Color32::from_rgba_unmultiplied(
                    30,
                    30,
                    32,
                    if translucent { 150 } else { 255 },
                ),
                border: Color32::from_white_alpha(30),
                text: Color32::from_rgb(0xf2, 0xf2, 0xf7),
                weak: Color32::from_rgb(0x98, 0x98, 0x9f),
                card: Color32::from_white_alpha(16),
                card_hover: Color32::from_white_alpha(26),
                card_stroke: Color32::from_white_alpha(14),
                field: Color32::from_white_alpha(20),
                track: Color32::from_gray(78),
                warn: Color32::from_rgb(0xff, 0xb3, 0x40),
                ok: Color32::from_rgb(0x34, 0xc7, 0x59),
            }
        } else {
            Self {
                accent,
                background: Color32::from_rgba_unmultiplied(
                    246,
                    246,
                    248,
                    if translucent { 160 } else { 255 },
                ),
                border: Color32::from_black_alpha(24),
                text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
                weak: Color32::from_rgb(0x86, 0x86, 0x8b),
                card: Color32::from_rgba_unmultiplied(255, 255, 255, 215),
                card_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 245),
                card_stroke: Color32::from_black_alpha(14),
                field: Color32::from_black_alpha(12),
                track: Color32::from_gray(214),
                warn: Color32::from_rgb(0xc2, 0x6a, 0x00),
                ok: Color32::from_rgb(0x28, 0xa7, 0x45),
            }
        }
    }
}

/// 卡片右上角的时间：今天显示时分，昨天显示“昨天”，更早显示日期。
pub fn card_time(ms: i64, now_ms: i64) -> String {
    let (Some(time), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    let days = (now.date_naive() - time.date_naive()).num_days();
    match days {
        0 => time.format("%H:%M").to_string(),
        1 => "昨天".into(),
        _ if time.format("%Y").to_string() == now.format("%Y").to_string() => {
            time.format("%-m月%-d日").to_string()
        }
        _ => time.format("%Y/%-m/%-d").to_string(),
    }
}

/// 2185 → "2,185"
pub fn group_digits(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

pub fn kind_color(kind: ContentKind) -> Color32 {
    match kind {
        ContentKind::Text => Color32::from_rgb(0x6b, 0x7c, 0x93),
        ContentKind::RichText => Color32::from_rgb(0x8e, 0x6c, 0xd8),
        ContentKind::Link => Color32::from_rgb(0x2f, 0x80, 0xed),
        ContentKind::Image => Color32::from_rgb(0x27, 0xae, 0x60),
        ContentKind::Files => Color32::from_rgb(0xe6, 0x7e, 0x22),
        ContentKind::Other => Color32::from_rgb(0x95, 0xa5, 0xa6),
    }
}

pub fn kind_icon(kind: ContentKind) -> Icon {
    match kind {
        ContentKind::Text | ContentKind::RichText => Icon::Document,
        ContentKind::Link => Icon::Link,
        ContentKind::Image => Icon::Image,
        ContentKind::Files => Icon::Folder,
        ContentKind::Other => Icon::Box,
    }
}

pub fn absolute_time(ms: i64) -> String {
    local(ms).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn local(ms: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&chrono::Local))
}

pub fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_card_time_and_counts() {
        let now = chrono::Local::now().timestamp_millis();
        assert_eq!(card_time(now, now).len(), 5, "今天显示 HH:MM");
        assert_eq!(card_time(now - 86_400_000, now), "昨天");
        assert_eq!(group_digits(2185), "2,185");
        assert_eq!(group_digits(1_000_000), "1,000,000");
        assert_eq!(group_digits(12), "12");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(byte_size(512), "512 B");
        assert_eq!(byte_size(1536), "1.5 KB");
        assert_eq!(byte_size(5 * 1024 * 1024), "5.0 MB");
    }
}
