//! `type:` 过滤器的扩展名映射表（§7.8）。
//!
//! 映射表固定在这里；后续扩展只改本表，不在解析器里硬编码。
//! `picture` 的集合按设计文档固定：png、jpg、jpeg、gif、bmp、webp、tiff、heic、avif。

/// 类型组名 → 扩展名列表（不含点，小写）。
pub fn extensions_for(group: &str) -> Option<&'static [&'static str]> {
    match group.to_ascii_lowercase().as_str() {
        "picture" | "photo" | "image" => Some(&[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "heic", "avif",
        ]),
        "video" => Some(&["mp4", "mov", "avi", "mkv", "webm", "m4v"]),
        "audio" => Some(&["mp3", "wav", "aac", "flac", "m4a", "ogg"]),
        "archive" => Some(&["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "tgz"]),
        "doc" => Some(&["pdf", "doc", "docx", "txt", "md", "rtf", "xls", "xlsx", "ppt", "pptx"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picture_group_is_fixed() {
        let picture = super::extensions_for("picture").unwrap();
        assert_eq!(picture, ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "heic", "avif"]);
        assert!(super::extensions_for("nope").is_none());
        // 组名不区分大小写。
        assert!(super::extensions_for("PICTURE").is_some());
    }
}
