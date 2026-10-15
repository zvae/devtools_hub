use serde::{Deserialize, Serialize};

/// 剪贴板中某一种类型的数据，对应 `NSPasteboardItem` 上的一个 type（UTI）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Representation {
    pub uti: String,
    pub data: Vec<u8>,
}

impl Representation {
    pub fn new(uti: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            uti: uti.into(),
            data: data.into(),
        }
    }
}

/// 一次完整的剪贴板快照。
///
/// 系统剪贴板可以同时包含多个 item（例如在 Finder 里复制多个文件），
/// 每个 item 又可以带多种表示（纯文本 / RTF / HTML / 图片……）。
/// 保存全部表示，粘贴时原样写回，才能做到“保真粘贴”。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<Vec<Representation>>,
}

impl Snapshot {
    pub fn from_text(text: &str) -> Self {
        Self {
            items: vec![vec![Representation::new(
                crate::uti::PLAIN_TEXT,
                text.as_bytes(),
            )]],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.iter().all(|item| item.is_empty())
    }

    pub fn total_bytes(&self) -> usize {
        self.representations().map(|rep| rep.data.len()).sum()
    }

    pub fn representations(&self) -> impl Iterator<Item = &Representation> {
        self.items.iter().flatten()
    }

    pub fn has_type(&self, uti: &str) -> bool {
        self.representations().any(|rep| rep.uti == uti)
    }

    /// 第一个包含该类型的 item 上的数据。
    pub fn first(&self, uti: &str) -> Option<&[u8]> {
        self.representations()
            .find(|rep| rep.uti == uti)
            .map(|rep| rep.data.as_slice())
    }

    /// 所有 item 上该类型的数据（多文件复制时每个文件一个 item）。
    pub fn all<'a>(&'a self, uti: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.representations()
            .filter(move |rep| rep.uti == uti)
            .map(|rep| rep.data.as_slice())
    }
}

/// 条目的内容类型，决定列表图标、预览方式和去重时取哪份数据做指纹。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentKind {
    Text,
    RichText,
    Link,
    Image,
    Files,
    Other,
}

impl ContentKind {
    pub const ALL: [ContentKind; 6] = [
        ContentKind::Text,
        ContentKind::RichText,
        ContentKind::Link,
        ContentKind::Image,
        ContentKind::Files,
        ContentKind::Other,
    ];

    pub fn as_i64(self) -> i64 {
        match self {
            ContentKind::Text => 0,
            ContentKind::RichText => 1,
            ContentKind::Link => 2,
            ContentKind::Image => 3,
            ContentKind::Files => 4,
            ContentKind::Other => 5,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        match value {
            0 => ContentKind::Text,
            1 => ContentKind::RichText,
            2 => ContentKind::Link,
            3 => ContentKind::Image,
            4 => ContentKind::Files,
            _ => ContentKind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContentKind::Text => "文本",
            ContentKind::RichText => "富文本",
            ContentKind::Link => "链接",
            ContentKind::Image => "图片",
            ContentKind::Files => "文件",
            ContentKind::Other => "其他",
        }
    }

    /// 是否属于“文本类”（筛选“文本”时同时包含纯文本和富文本）。
    pub fn is_textual(self) -> bool {
        matches!(self, ContentKind::Text | ContentKind::RichText)
    }
}

/// 复制发生时的前台应用。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceApp {
    pub bundle_id: Option<String>,
    pub name: Option<String>,
}

impl SourceApp {
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.bundle_id.as_deref())
            .unwrap_or("未知应用")
    }
}

/// 监听线程捕获到的一次复制。
#[derive(Clone, Debug)]
pub struct Capture {
    pub snapshot: Snapshot,
    pub source: SourceApp,
    pub captured_at_ms: i64,
}

/// 列表中展示的条目元数据（不含完整数据，完整数据按需从存储读取）。
#[derive(Clone, Debug, PartialEq)]
pub struct ClipItem {
    pub id: i64,
    pub kind: ContentKind,
    pub title: String,
    pub source: SourceApp,
    pub created_at_ms: i64,
    pub last_used_at_ms: i64,
    pub copy_count: i64,
    pub paste_count: i64,
    pub byte_size: i64,
    pub file_count: i64,
    pub image_size: Option<(u32, u32)>,
    pub has_thumbnail: bool,
    pub list_ids: Vec<i64>,
}

impl ClipItem {
    pub fn in_list(&self, list_id: i64) -> bool {
        self.list_ids.contains(&list_id)
    }
}

/// 用户列表（PasteNow 中的 “Normal List”）。列表里的条目不会被历史清理删除。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipList {
    pub id: i64,
    pub name: String,
    pub item_count: i64,
}
