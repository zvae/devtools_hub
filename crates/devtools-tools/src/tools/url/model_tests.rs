//! URL 编码规则与浏览器行为对照，以及非法转义的报错。

use super::*;

const LINK: &str = "https://example.com/搜索?q=a b&lang=zh";

#[test]
fn encodes_like_browser_functions() {
    assert_eq!(
        encode(LINK, 0).unwrap(),
        "https%3A%2F%2Fexample.com%2F%E6%90%9C%E7%B4%A2%3Fq%3Da%20b%26lang%3Dzh"
    );
    assert_eq!(
        encode(LINK, 1).unwrap(),
        "https://example.com/%E6%90%9C%E7%B4%A2?q=a%20b&lang=zh"
    );
    assert_eq!(
        encode("a b&c=中/文*~!'()", 0).unwrap(),
        "a%20b%26c%3D%E4%B8%AD%2F%E6%96%87*~!'()"
    );
    assert_eq!(
        encode("a b&c=中/文*~", 2).unwrap(),
        "a+b%26c%3D%E4%B8%AD%2F%E6%96%87*%7E"
    );
}

#[test]
fn decodes_and_round_trips() {
    for variant in 0..3 {
        assert_eq!(
            decode(&encode(LINK, variant).unwrap(), variant).unwrap(),
            LINK
        );
    }
    assert_eq!(decode("%e4%b8%ad", 0).unwrap(), "中");
    assert_eq!(decode("a+b", 0).unwrap(), "a+b");
    assert_eq!(decode("a+b", 2).unwrap(), "a b");
}

#[test]
fn rejects_invalid_escapes() {
    for text in ["%", "%E", "%ZZ", "50%", "%E4%B8"] {
        assert!(decode(text, 0).is_err(), "{text}");
    }
    assert!(decode("中%G1", 0).unwrap_err().starts_with("第 2 个字符"));
}

#[test]
fn detects_percent_encoded_text() {
    assert_eq!(detect("%E4%B8%AD%E6%96%87"), Some(0));
    assert_eq!(detect("q=a+b%26c"), Some(2));
    for text in ["hello", "100%", "{}", "1700000000", "%E4%B8"] {
        assert_eq!(detect(text), None, "{text}");
    }
}
