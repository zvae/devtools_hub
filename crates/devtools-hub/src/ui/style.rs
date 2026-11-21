//! 面板配色与格式化工具。
//!
//! 基础样式（配色 `Palette`、圆角 / 留白规范、连续曲率圆角、字体安装、输入框）来自
//! 共用模块 `devtools-ui`，此处再导出以保持 `ui::style` 的调用路径不变；只有与剪贴板内容
//! 相关的代表色 / 图标，以及时间、数字、大小的格式化留在本模块。

use devtools_clipboard::ContentKind;
use egui::Color32;

use super::icons::Icon;

pub use devtools_ui::style::*;

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

/// 内容类型的代表色（没有来源应用图标时的类型徽标）。
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

/// 内容类型的图标。
pub fn kind_icon(kind: ContentKind) -> Icon {
    match kind {
        ContentKind::Text | ContentKind::RichText => Icon::Document,
        ContentKind::Link => Icon::Link,
        ContentKind::Image => Icon::Image,
        ContentKind::Files => Icon::Folder,
        ContentKind::Other => Icon::Box,
    }
}

/// 本地时间，例如 `2024-05-01 09:30:00`（预览里的复制时间）。
pub fn absolute_time(ms: i64) -> String {
    local(ms).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// 毫秒时间戳转成本地时区的时间。
fn local(ms: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&chrono::Local))
}

/// 人类可读的大小，例如 `1.5 MB`。
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
