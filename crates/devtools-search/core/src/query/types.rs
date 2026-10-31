//! `type:` 过滤器的扩展名映射表（§7.8）与结果区的类型筛选（[`FileKind`]）。
//!
//! 映射表固定在这里；后续扩展只改本表，不在解析器里硬编码。
//! `picture` 的集合按设计文档固定：png、jpg、jpeg、gif、bmp、webp、tiff、heic、avif。

use serde::{Deserialize, Serialize};

use super::ast::{Ast, Term};

/// 类型组名 → 扩展名列表（不含点，小写）。
pub fn extensions_for(group: &str) -> Option<&'static [&'static str]> {
    match group.to_ascii_lowercase().as_str() {
        "picture" | "photo" | "image" => Some(&[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "heic", "avif",
        ]),
        "video" => Some(&[
            "mp4", "mov", "avi", "mkv", "webm", "m4v", "mpg", "mpeg", "wmv", "flv",
        ]),
        "audio" => Some(&[
            "mp3", "wav", "aac", "flac", "m4a", "ogg", "wma", "aiff", "opus", "mid",
        ]),
        "archive" => Some(&[
            "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "tgz", "zst", "iso", "cab",
        ]),
        "doc" => Some(&[
            "pdf", "doc", "docx", "txt", "md", "rtf", "xls", "xlsx", "ppt", "pptx", "csv",
            "odt", "ods", "odp", "epub",
        ]),
        // 可执行与安装包（两个平台的常见形态）：Windows 的 exe/msi，macOS 的 app/dmg/pkg，
        // Linux 的 deb/rpm/AppImage，外加可以直接运行的脚本。
        "executable" | "app" => Some(&[
            "exe", "msi", "msix", "com", "app", "dmg", "pkg", "deb", "rpm", "appimage",
            "bat", "cmd", "ps1", "sh", "run",
        ]),
        _ => None,
    }
}

/// 结果区"类型"下拉的筛选项（参照 Everything 的类型过滤）。
///
/// 只是 [`extensions_for`] 与 `folder:` 的一个薄封装：界面选中某一项后，
/// 查询条件里 AND 上对应的 `type:` / `folder:` 项，因此可以和关键词、布尔语法、
/// `ext:` 等过滤器自由组合，而不是对已有结果做二次筛选。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// 所有（不过滤）。
    #[default]
    All,
    Audio,
    Archive,
    Document,
    Executable,
    Folder,
    Picture,
    Video,
}

impl FileKind {
    /// 下拉里的顺序（与 Everything 的类型列表一致：所有 → 音频 → 压缩 → 文档 → 可执行 → 文件夹 → 图片 → 视频）。
    pub const ALL: [FileKind; 8] = [
        FileKind::All,
        FileKind::Audio,
        FileKind::Archive,
        FileKind::Document,
        FileKind::Executable,
        FileKind::Folder,
        FileKind::Picture,
        FileKind::Video,
    ];

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            FileKind::All => "所有",
            FileKind::Audio => "音频",
            FileKind::Archive => "压缩文件",
            FileKind::Document => "文档",
            FileKind::Executable => "可执行文件",
            FileKind::Folder => "文件夹",
            FileKind::Picture => "图片",
            FileKind::Video => "视频",
        }
    }

    /// 对应的 `type:` 组名（`Folder` / `All` 没有组名）。
    fn group(self) -> Option<&'static str> {
        match self {
            FileKind::All | FileKind::Folder => None,
            FileKind::Audio => Some("audio"),
            FileKind::Archive => Some("archive"),
            FileKind::Document => Some("doc"),
            FileKind::Executable => Some("executable"),
            FileKind::Picture => Some("picture"),
            FileKind::Video => Some("video"),
        }
    }

    /// 把筛选翻成查询条件：`All` 返回 `None`（不加条件），`Folder` 用 `folder:`。
    pub fn to_ast(self) -> Option<Ast> {
        match self {
            Self::All => None,
            Self::Folder => Some(Ast::Term(Term::Folder)),
            other => other
                .group()
                .map(|group| Ast::Term(Term::Type(group.to_string()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_group_is_fixed() {
        let picture = extensions_for("picture").unwrap();
        assert_eq!(picture, ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "heic", "avif"]);
        assert!(extensions_for("nope").is_none());
        // 组名不区分大小写。
        assert!(extensions_for("PICTURE").is_some());
    }

    #[test]
    fn executable_group_covers_both_platforms() {
        let exe = extensions_for("executable").unwrap();
        for ext in ["exe", "msi", "app", "dmg", "sh", "ps1"] {
            assert!(exe.contains(&ext), "缺少扩展名：{ext}");
        }
    }

    #[test]
    fn every_kind_maps_to_a_known_group() {
        for kind in FileKind::ALL {
            // 每个筛选项要么不加条件，要么翻成能求值的条件（组名必须在映射表里）。
            if let Some(group) = kind.group() {
                assert!(
                    extensions_for(group).is_some(),
                    "{} 的组 {} 没有扩展名表",
                    kind.label(),
                    group
                );
            }
            assert_eq!(kind.to_ast().is_none(), kind == FileKind::All);
        }
        assert_eq!(FileKind::Folder.to_ast(), Some(Ast::Term(Term::Folder)));
    }

    #[test]
    fn kind_serializes_as_snake_case() {
        assert_eq!(serde_json::to_string(&FileKind::All).unwrap(), "\"all\"");
        assert_eq!(serde_json::to_string(&FileKind::Picture).unwrap(), "\"picture\"");
        assert_eq!(
            serde_json::from_str::<FileKind>("\"document\"").unwrap(),
            FileKind::Document
        );
    }
}
