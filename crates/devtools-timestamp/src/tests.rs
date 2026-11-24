//! 时间戳转换的边界与往返测试。

use super::*;

#[test]
fn epoch_and_negative_milliseconds_round_trip() {
    assert_eq!(
        format_date(from_timestamp("0", Unit::Seconds).unwrap()),
        "1970-01-01 08:00:00.000"
    );
    for value in ["-1", "-1001", "1700000000123", "0"] {
        let date = from_timestamp(value, Unit::Milliseconds).unwrap();
        assert_eq!(
            Unit::Milliseconds
                .value(from_date(&format_date(date)).unwrap())
                .to_string(),
            value
        );
    }
    assert_eq!(
        Unit::Seconds.value(from_timestamp("-1", Unit::Milliseconds).unwrap()),
        -1
    );
}

#[test]
fn date_is_beijing_independent_of_system_timezone() {
    assert_eq!(from_date("1970-01-01 08:00:00").unwrap().timestamp(), 0);
    assert_eq!(from_date("1970-01-01T00:00:00Z").unwrap().timestamp(), 0);
    assert_eq!(
        from_date("1970-01-01T08:00:00+08:00").unwrap().timestamp(),
        0
    );
    assert_eq!(
        from_date("2024-02-29 00:00:00.123")
            .unwrap()
            .timestamp_subsec_millis(),
        123
    );
}

#[test]
fn rejects_invalid_and_out_of_range_inputs() {
    for text in [
        "",
        "-",
        "1.5",
        "123abc",
        "9223372036854775807",
        "-9223372036854775808",
    ] {
        assert!(from_timestamp(text, Unit::Seconds).is_err(), "{text}");
        assert!(from_timestamp(text, Unit::Milliseconds).is_err(), "{text}");
    }
    for text in [
        "2023-02-29 00:00:00",
        "2024-01-01 25:00:00",
        "hello",
        "2016-12-31T23:59:60Z",
        "2016-12-31 23:59:60",
    ] {
        assert!(from_date(text).is_err());
    }
    assert_eq!(detect(" 1700000000 "), Some(Unit::Seconds));
    assert_eq!(detect("1700000000123"), Some(Unit::Milliseconds));
    assert!(!matches("{\"time\":1700000000}"));
}
