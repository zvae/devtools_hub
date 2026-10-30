//! 匹配文本的归一化规则（§7.1）。
//!
//! - 大小写不敏感文本：Unicode NFC → Rust Unicode lowercase → NFC。
//!   这不是完整 Unicode case folding（例如 ß 与 ss 不保证等价），是 v1 的明确选择。
//! - Aa 开启（区分大小写）仅做 NFC，保留大小写。

use unicode_normalization::UnicodeNormalization;

/// NFC 归一化。
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// 不区分大小写文本：NFC → lowercase → NFC（§7.1）。
pub fn fold(text: &str) -> String {
    text.to_lowercase().nfc().collect()
}

/// 按查询的大小写设置准备匹配文本：区分时 NFC，不区分时 fold。
pub fn match_text_of(text: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        nfc(text)
    } else {
        fold(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_normalization::UnicodeNormalization;

    #[test]
    fn folds_ascii_and_cjk() {
        // CJK 下 raw == NFC == lower，三份文本共享（§7.1）。
        assert_eq!(fold("文件Name.TXT"), "文件name.txt");
        assert_eq!(nfc("文件Name.TXT"), "文件Name.TXT");
    }

    #[test]
    fn nfc_composes_decomposed() {
        // 韩文组合字符（U+AE30）与分解字符 NFC 后相同。
        let composed = "\u{ae30}";
        let decomposed = composed.nfd().collect::<String>();
        assert_ne!(composed, decomposed);
        assert_eq!(nfc(&decomposed), composed);
    }

    #[test]
    fn fold_is_not_full_case_folding() {
        // ß 与 ss 不等价（§7.1 的明确限制，不是缺陷）。
        assert_ne!(fold("ß"), fold("ss"));
    }
}
