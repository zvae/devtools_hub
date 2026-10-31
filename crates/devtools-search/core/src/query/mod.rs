//! 查询：语法解析、求值与排序（§7.8、§7.9）。
//!
//! v1 的公开支持集合以设计文档 §7.8 为准：子串/短语、通配符、布尔（AND/OR/NOT/分组）、
//! 路径分段、`ext:` / `type:` / `folder:` / `file:` / `size:` / `dm:` / `dc:` /
//! `infolder:` / `parent:` / `regex:`。入口带来的原始文本走字面短语（`query_is_literal`），
//! 不解析布尔/过滤器语法。

mod ast;
mod eval;
mod parse;
mod sort;
mod types;

pub use ast::{Ast, DateFilter, PathSegment, SizeFilter, Term};
pub use eval::{compile, eval, EvalOptions, EvalOutcome, Notice};
pub use parse::{parse, ParseError};
pub use sort::sort_matches;
pub use types::{extensions_for, FileKind};

#[cfg(test)]
pub(crate) use eval::wildcard_match;

#[cfg(test)]
mod tests;