//! 查询抽象语法树（§7.8）。

use crate::model::NodeId;

/// 查询的布尔结构。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ast {
    /// 空格分隔的 AND；空向量匹配全部。
    And(Vec<Ast>),
    Or(Vec<Ast>),
    Not(Box<Ast>),
    Term(Term),
}

/// 一个叶子条件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// 文件名子串（无通配符时）或整名通配符匹配（含 `*`/`?` 时）。
    /// 引号短语也是一种子串（保留空格、不解释通配符）。
    Name { pattern: String, wildcards: bool },
    /// 路径分段匹配（`project/report`）：先匹配最后一段名字，再沿 parent 链逐段验证。
    Path(Vec<PathSegment>),
    /// `ext:pdf;txt`（扩展名列表，不区分大小写）。
    Ext(Vec<String>),
    /// `type:picture`（扩展名类型组，映射表在 [`super::types`]）。
    Type(String),
    /// `file:`。
    File,
    /// `folder:`。
    Folder,
    /// `size:>1MB` / `size:<10KB` / `size:100KB-5MB` / `size:1MB`（1024 进制）。
    Size(SizeFilter),
    /// `dm:`（修改时间过滤，本地时区边界）。
    DateModified(DateFilter),
    /// `dc:`（创建时间过滤）。
    DateCreated(DateFilter),
    /// `infolder:~/Documents`：全部后代（解析成目录节点，DFS 区间判断）。
    InFolder(String),
    /// `parent:~/Downloads`：直接子项（比较父节点）。
    Parent(String),
    /// `regex:^report`：对名字执行正则；非法表达式在解析时报错。
    Regex(String),
}

/// 路径分段中的一段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSegment {
    pub pattern: String,
    pub wildcards: bool,
}

/// 大小过滤。单位固定 1024 进制：`1KB=1024B`、`1MB=1024KB`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeFilter {
    Gt(u64),
    Lt(u64),
    Range(u64, u64),
    Exact(u64),
}

/// 时间过滤（Unix 秒，`[start, end)`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateFilter {
    pub start: i64,
    pub end: i64,
}

impl DateFilter {
    /// 时间戳是否命中（§7.8：Unknown 不命中需要该字段的过滤器）。
    pub fn contains(self, ts: Option<i64>) -> bool {
        let Some(ts) = ts else { return false };
        ts >= self.start && ts < self.end
    }
}

/// 求值上下文里解析好的目录引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirRef {
    /// 指向索引中的目录节点。
    Node(NodeId),
    /// 目录不在索引范围内（§7.8：返回空结果并提示，不报错、不自动加根）。
    NotIndexed,
}
