//! UTF-8 三种字节格式的往返、宽松解析与误判防护测试。

use super::*;

#[test]
fn encodes_three_formats_and_round_trips() {
    assert_eq!(encode("中文", 0).unwrap(), "\\xE4\\xB8\\xAD\\xE6\\x96\\x87");
    assert_eq!(encode("中文", 1).unwrap(), "E4 B8 AD E6 96 87");
    assert_eq!(encode("中文", 2).unwrap(), "228, 184, 173, 230, 150, 135");
    assert_eq!(encode("", 1).unwrap(), "");
    for text in ["A", "héllo 世界 🎉", "line\nbreak"] {
        for variant in 0..3 {
            assert_eq!(
                decode(&encode(text, variant).unwrap(), variant).unwrap(),
                text
            );
        }
    }
}

#[test]
fn lenient_parsing() {
    assert_eq!(decode("abc\\xe4\\xb8\\xad", 0).unwrap(), "abc中");
    assert_eq!(decode("E4B8AD", 1).unwrap(), "中");
    assert_eq!(decode("0xE4, 0xB8, 0xAD", 1).unwrap(), "中");
    assert_eq!(decode("[228, 184, 173]", 2).unwrap(), "中");
}

#[test]
fn rejects_invalid_input() {
    assert!(decode("\\xE4\\xB8", 0)
        .unwrap_err()
        .contains("第 1 个字节起"));
    assert!(decode("\\xZZ", 0).is_err());
    assert!(decode("E4 B", 1).is_err());
    assert!(decode("GG", 1).is_err());
    assert!(decode("256", 2).is_err());
    assert!(decode("-1", 2).is_err());
}

#[test]
fn detects_byte_sequences_only() {
    assert_eq!(detect("\\xE4\\xB8\\xAD"), Some(0));
    assert_eq!(detect("E4 B8 AD E6 96 87"), Some(1));
    assert_eq!(detect("228, 184, 173"), Some(2));
    for text in ["hello", "1700000000", "{}", "12 34", "中文", "deadbeef"] {
        assert_eq!(detect(text), None, "{text}");
    }
}
