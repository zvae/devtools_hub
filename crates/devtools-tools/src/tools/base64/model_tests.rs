//! Base64 的标准向量、字母表与非法输入测试。

use super::*;

#[test]
fn rfc4648_vectors_round_trip() {
    for (text, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
        ("中文", "5Lit5paH"),
        ("你好，DevTools Hub", "5L2g5aW977yMRGV2VG9vbHMgSHVi"),
    ] {
        assert_eq!(encode(text, 0).unwrap(), encoded);
        assert_eq!(decode(encoded, 0).unwrap(), text);
    }
}

#[test]
fn url_safe_alphabet_without_padding() {
    assert_eq!(encode_bytes(&[0xfb, 0xff], false), "+/8=");
    assert_eq!(encode_bytes(&[0xfb, 0xff], true), "-_8");
    assert_eq!(encode(">>>???", 0).unwrap(), "Pj4+Pz8/");
    assert_eq!(encode(">>>???", 1).unwrap(), "Pj4-Pz8_");
    assert_eq!(encode("f", 1).unwrap(), "Zg");
    // 解码不区分字母表，填充可省略。
    assert_eq!(decode("Pj4-Pz8_", 0).unwrap(), ">>>???");
    assert_eq!(decode("Zg", 0).unwrap(), "f");
    assert_eq!(decode_bytes("-_8").unwrap(), [0xfb, 0xff]);
}

#[test]
fn ignores_whitespace_and_rejects_invalid_input() {
    assert_eq!(decode(" Zm9v\nYmFy\r\n", 0).unwrap(), "foobar");
    for text in ["Zm9vY", "Zm9v=Zg==", "Zg===", "Zm8==", "@@@@", "Zm9v!"] {
        assert!(decode(text, 0).is_err(), "{text}");
    }
    // 合法 Base64 但不是 UTF-8 文本。
    assert!(decode("+/8=", 0).is_err());
}

#[test]
fn detects_only_readable_base64() {
    assert_eq!(detect("5L2g5aW977yMRGV2VG9vbHMgSHVi"), Some(0));
    assert_eq!(detect(" Zm9vYmFy\n"), Some(0));
    assert_eq!(detect("Pj4-Pz8_"), Some(1));
    for text in [
        "hello",
        "test",
        "1700000000",
        "{}",
        "+/8=",
        "Zm9v YmFy",
        "abcd1234",
    ] {
        assert_eq!(detect(text), None, "{text}");
    }
}
