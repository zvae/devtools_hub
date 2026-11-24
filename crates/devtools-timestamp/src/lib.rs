//! 时间戳与日期双向转换：复用 chrono，全部离线处理，不依赖系统时区。
//! 默认采用固定 UTC+08:00（北京时间）；显式偏移的 ISO 8601 输入尊重其自身时区。
//! 与 devtools-json 一样，不依赖界面与平台，可单独运行转换与边界测试。

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};

/// Unix 时间戳的输入或输出单位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    Seconds,
    #[default]
    Milliseconds,
}

impl Unit {
    /// 单位的界面说明。
    pub fn label(self) -> &'static str {
        match self {
            Self::Seconds => "秒 (s)",
            Self::Milliseconds => "毫秒 (ms)",
        }
    }

    pub fn value(self, date: DateTime<Utc>) -> i64 {
        match self {
            Self::Seconds => date.timestamp(),
            Self::Milliseconds => date.timestamp_millis(),
        }
    }
}

/// 默认日期展示始终保留三位毫秒，避免双向转换时无声丢失精度。
pub fn format_date(date: DateTime<Utc>) -> String {
    date.with_timezone(&beijing())
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).unwrap()
}

/// 解析整数时间戳，并拒绝超范围或无法显示为北京时间的值。
pub fn from_timestamp(text: &str, unit: Unit) -> Result<DateTime<Utc>, &'static str> {
    let text = text.trim();
    if !is_integer(text) {
        return Err("请输入整数时间戳，可使用负数表示 1970 年以前的时间");
    }
    let value = text.parse::<i64>().map_err(|_| "时间戳超出可转换范围")?;
    let date = match unit {
        Unit::Seconds => DateTime::from_timestamp(value, 0),
        Unit::Milliseconds => DateTime::from_timestamp_millis(value),
    };
    // 确保 UTC+08:00 仍在 chrono 可表示的范围内，格式化时不会溢出。
    date.filter(|date| {
        date.naive_utc()
            .checked_add_signed(chrono::Duration::hours(8))
            .is_some()
    })
    .ok_or("时间戳超出可转换范围")
}

/// 解析北京时间或带偏移的 ISO 8601 日期；不接受闰秒。
pub fn from_date(text: &str) -> Result<DateTime<Utc>, &'static str> {
    let text = text.trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        if date.timestamp_subsec_nanos() < 1_000_000_000 {
            return Ok(date.with_timezone(&Utc));
        }
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(date) = NaiveDateTime::parse_from_str(text, format) {
            // Unix 时间戳不表示闰秒，避免与普通时间戳发生混淆。
            if date.and_utc().timestamp_subsec_nanos() >= 1_000_000_000 {
                break;
            }
            return beijing()
                .from_local_datetime(&date)
                .single()
                .map(|date| date.with_timezone(&Utc))
                .ok_or("日期超出可转换范围");
        }
    }
    Err("请输入有效日期，例如 2026-11-23 12:30:00.123，或带时区的 ISO 8601 日期")
}

fn is_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// 自动带入时按位数识别秒／毫秒；手动转换则以用户选定的单位为准。
pub fn detect(text: &str) -> Option<Unit> {
    let text = text.trim();
    if !is_integer(text) {
        return None;
    }
    let digits = text.trim_start_matches('-').len();
    let unit = if digits <= 10 {
        Unit::Seconds
    } else {
        Unit::Milliseconds
    };
    from_timestamp(text, unit).ok().map(|_| unit)
}

/// 判断选中文本能否直接带入时间戳转换器。
pub fn matches(text: &str) -> bool {
    detect(text).is_some() || from_date(text).is_ok()
}

#[cfg(test)]
mod tests;
