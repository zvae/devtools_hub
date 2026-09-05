use super::*;

pub(super) fn convert_timestamp(input: &str) -> CommandResult {
    let value = input.trim();
    let content = if value.is_empty() {
        format!(
            "Unix: {}\n+08:00: {}",
            timestamp_now(),
            timestamp_now_datetime()
        )
    } else if let Ok(seconds) = value.parse::<i64>() {
        timestamp_to_datetime(seconds)
            .map(|datetime| format!("Unix: {seconds}\n+08:00: {datetime}"))
            .unwrap_or_else(|error| format!("Invalid Unix timestamp: {error}"))
    } else {
        timestamp_from_datetime(value)
            .map(|timestamp| format!("Unix: {timestamp}\n+08:00 input"))
            .unwrap_or_else(|error| format!("Invalid +08:00 datetime: {error}"))
    };
    CommandResult {
        title: "Timestamp Convert".into(),
        content,
    }
}

/// 返回当前秒级或毫秒级时间戳。时间戳本身始终基于 UTC，因此无需时区换算。
pub fn timestamp_now() -> String {
    timestamp_now_with_unit(true)
}

pub fn timestamp_now_with_unit(milliseconds: bool) -> String {
    let now = time::OffsetDateTime::now_utc();
    if milliseconds {
        (now.unix_timestamp_nanos() / 1_000_000).to_string()
    } else {
        now.unix_timestamp().to_string()
    }
}

/// 将当前时间格式化为固定 +08:00 时区的本地日期时间。
pub fn timestamp_now_datetime() -> String {
    format_east8(time::OffsetDateTime::now_utc())
}

/// 将 Unix 秒级时间戳转换为 +08:00 日期时间。
pub fn timestamp_to_datetime(seconds: i64) -> Result<String, String> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .map(format_east8)
        .map_err(|error| error.to_string())
}

/// 自动识别秒级和毫秒级时间戳。13 位左右的现代时间戳按毫秒处理。
pub fn timestamp_to_datetime_auto(value: i64) -> Result<String, String> {
    let seconds = if value.unsigned_abs() >= 100_000_000_000 {
        value / 1_000
    } else {
        value
    };
    timestamp_to_datetime(seconds)
}

/// 将固定 +08:00 的 `YYYY-MM-DD HH:MM:SS` 日期时间转换为 Unix 秒级时间戳。
pub fn timestamp_from_datetime(value: &str) -> Result<String, String> {
    use time::{format_description::FormatItem, macros::format_description, PrimitiveDateTime};

    static FORMAT: &[FormatItem<'static>] =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value.trim(), FORMAT)
        .map(|value| {
            value
                .assume_offset(east8_offset())
                .unix_timestamp()
                .to_string()
        })
        .map_err(|error| error.to_string())
}

pub fn timestamp_from_datetime_with_unit(
    value: &str,
    milliseconds: bool,
) -> Result<String, String> {
    timestamp_from_datetime(value).and_then(|seconds| convert_timestamp_unit(seconds, milliseconds))
}

/// 将分段的 +08:00 日期时间转换为 Unix 秒级时间戳。
pub fn timestamp_from_parts(
    year: &str,
    month: &str,
    day: &str,
    hour: &str,
    minute: &str,
    second: &str,
) -> Result<String, String> {
    use time::{Date, Month, PrimitiveDateTime, Time};

    let year = year
        .trim()
        .parse::<i32>()
        .map_err(|_| "year must be a number")?;
    let month = month
        .trim()
        .parse::<u8>()
        .map_err(|_| "month must be a number")?;
    let day = day
        .trim()
        .parse::<u8>()
        .map_err(|_| "day must be a number")?;
    let hour = hour
        .trim()
        .parse::<u8>()
        .map_err(|_| "hour must be a number")?;
    let minute = minute
        .trim()
        .parse::<u8>()
        .map_err(|_| "minute must be a number")?;
    let second = second
        .trim()
        .parse::<u8>()
        .map_err(|_| "second must be a number")?;
    let month = Month::try_from(month).map_err(|error| error.to_string())?;
    let date = Date::from_calendar_date(year, month, day).map_err(|error| error.to_string())?;
    let time = Time::from_hms(hour, minute, second).map_err(|error| error.to_string())?;
    Ok(PrimitiveDateTime::new(date, time)
        .assume_offset(east8_offset())
        .unix_timestamp()
        .to_string())
}

pub fn timestamp_from_parts_with_unit(
    year: &str,
    month: &str,
    day: &str,
    hour: &str,
    minute: &str,
    second: &str,
    milliseconds: bool,
) -> Result<String, String> {
    timestamp_from_parts(year, month, day, hour, minute, second)
        .and_then(|seconds| convert_timestamp_unit(seconds, milliseconds))
}

pub(super) fn convert_timestamp_unit(
    seconds: String,
    milliseconds: bool,
) -> Result<String, String> {
    if !milliseconds {
        return Ok(seconds);
    }
    seconds
        .parse::<i64>()
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .checked_mul(1_000)
                .map(|value| value.to_string())
                .ok_or_else(|| "timestamp is out of range".to_string())
        })
}

pub(super) fn east8_offset() -> time::UtcOffset {
    time::UtcOffset::from_hms(8, 0, 0).expect("+08:00 must be a valid UTC offset")
}

pub(super) fn format_east8(value: time::OffsetDateTime) -> String {
    use time::{format_description::FormatItem, macros::format_description};

    static FORMAT: &[FormatItem<'static>] =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    value
        .to_offset(east8_offset())
        .format(FORMAT)
        .unwrap_or_default()
}
