//! 查询语法解析（§7.8）。
//!
//! 文法（优先级 NOT > AND > OR，空格即 AND）：
//! ```text
//! expr    := or_expr
//! or_expr := and_expr ( '|' and_expr )*
//! and_expr := unary ( unary )*
//! unary   := '!' unary | primary
//! primary := '(' expr ')' | TERM
//! ```

use chrono::{Datelike, Local, NaiveDate, TimeZone};

use super::ast::{Ast, DateFilter, PathSegment, SizeFilter, Term};
use crate::text::match_text_of;

/// 解析错误（§7.8：输入不支持的过滤器报错，不悄悄忽略）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    Syntax(String),
    /// 未知的 `type:` 类型组。
    UnknownTypeGroup(String),
    /// 无法解析的 `size:` 值（单位固定 1024 进制）。
    BadSize(String),
    /// 无法解析的日期（`dm:` / `dc:`）。
    BadDate(String),
    /// 非法正则。
    BadRegex(String),
    /// v1 明确不支持的过滤器（避免悄悄忽略）。
    UnsupportedFilter(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Syntax(msg) => write!(f, "查询语法错误：{msg}"),
            ParseError::UnknownTypeGroup(g) => write!(f, "未知的类型组：type:{g}"),
            ParseError::BadSize(v) => write!(f, "无法解析的大小：size:{v}（单位 B/KB/MB/GB/TB，1024 进制）"),
            ParseError::BadDate(v) => write!(f, "无法解析的日期：{v}（可用 today/yesterday/thisweek/thismonth/thisyear/pastweek/pastmonth/pastyear 或 2026-01-31）"),
            ParseError::BadRegex(v) => write!(f, "非法正则：{v}"),
            ParseError::UnsupportedFilter(name) => write!(f, "暂不支持的过滤器：{name}:"),
        }
    }
}

impl std::error::Error for ParseError {}

/// 解析查询文本。空文本返回 `Ast::And(vec![])`（是否列出全部由 `show_all_on_empty` 决定）。
pub fn parse(query: &str) -> Result<Ast, ParseError> {
    let tokens = tokenize(query)?;
    if tokens.is_empty() {
        return Ok(Ast::And(Vec::new()));
    }
    let mut parser = Parser { tokens, pos: 0 };
    let ast = parser.parse_or()?;
    if parser.pos != parser.tokens.len() {
        return Err(ParseError::Syntax("多余的右括号".into()));
    }
    Ok(ast)
}

// ---- 词法 ----

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    LParen,
    RParen,
    Or,
    Not,
    /// 一个词（过滤器的值可能带空格，词法阶段已并入）。
    Word { value: String, quoted: bool },
}

/// 已知的过滤器前缀（`值` 跟在 `:` 后面）。
const FILTERS: &[&str] = &[
    "ext", "type", "file", "folder", "size", "dm", "dc", "infolder", "parent", "regex",
];

/// v1 明确不支持、遇到时显式报错的前缀（§7.8：不悄悄忽略）。
const UNSUPPORTED: &[&str] = &["path", "content", "dupe", "childcount"];

fn is_word_char(c: char) -> bool {
    !c.is_whitespace() && c != '(' && c != ')' && c != '|'
}

fn tokenize(query: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '|' => {
                tokens.push(Token::Or);
                i += 1;
            }
            '!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            '"' => {
                // 引号短语：保留空格，不解释通配符。
                let (value, next) = read_quoted(&chars, i);
                tokens.push(Token::Word { value, quoted: true });
                i = next;
            }
            _ => {
                let (word, next) = read_word(&chars, i);
                tokens.push(Token::Word { value: word, quoted: false });
                i = next;
            }
        }
    }
    Ok(tokens)
}

/// 读取一个裸词；若它是过滤器前缀且紧跟引号，把引号内容并入（`infolder:"My Docs"`）。
fn read_word(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    let mut word = String::new();
    while i < chars.len() && is_word_char(chars[i]) {
        word.push(chars[i]);
        i += 1;
    }
    // 过滤器前缀 + 引号值。
    if i < chars.len() && chars[i] == '"' {
        let lower = word.to_ascii_lowercase();
        let is_filter = FILTERS.iter().any(|f| {
            lower == *f || (lower.starts_with(f) && lower.as_bytes().get(f.len()) == Some(&b':'))
        });
        if is_filter {
            let (value, next) = read_quoted(chars, i);
            return (format!("{word}{value}"), next);
        }
    }
    (word, i)
}

/// 读取引号字符串（`i` 指向开引号）；返回 (内容含引号, 结束下标)。
fn read_quoted(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start + 1;
    let mut out = String::from("\"");
    while i < chars.len() && chars[i] != '"' {
        out.push(chars[i]);
        i += 1;
    }
    if i < chars.len() {
        out.push('"');
        i += 1;
    }
    (out, i)
}

// ---- 语法 ----

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_or(&mut self) -> Result<Ast, ParseError> {
        let mut terms = vec![self.parse_and()?];
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Ast::Or(terms)
        })
    }

    fn parse_and(&mut self) -> Result<Ast, ParseError> {
        let mut terms = Vec::new();
        while let Some(token) = self.peek() {
            match token {
                Token::Or | Token::RParen => break,
                _ => terms.push(self.parse_unary()?),
            }
        }
        if terms.is_empty() {
            return Err(ParseError::Syntax("表达式不完整".into()));
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Ast::And(terms)
        })
    }

    fn parse_unary(&mut self) -> Result<Ast, ParseError> {
        if self.peek() == Some(&Token::Not) {
            self.pos += 1;
            return Ok(Ast::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Ast, ParseError> {
        match self.peek() {
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.parse_or()?;
                if self.peek() != Some(&Token::RParen) {
                    return Err(ParseError::Syntax("缺少右括号".into()));
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(Token::Word { .. }) => {
                let Token::Word { value, quoted } = self.tokens[self.pos].clone() else {
                    unreachable!()
                };
                self.pos += 1;
                Ok(Ast::Term(term_from_word(&value, quoted)?))
            }
            _ => Err(ParseError::Syntax("表达式不完整（缺少条件）".into())),
        }
    }
}

/// 把一个词分类成叶子条件。
fn term_from_word(word: &str, quoted: bool) -> Result<Term, ParseError> {
    if quoted {
        // 引号短语：剥掉引号，按字面子串（保留空格、不解释通配符）。
        let value = word.trim_matches('"');
        return Ok(Term::Name { pattern: value.to_string(), wildcards: false });
    }
    // 过滤器前缀（不区分大小写）。
    let lower = word.to_ascii_lowercase();
    for name in UNSUPPORTED {
        if lower.starts_with(&format!("{name}:")) {
            return Err(ParseError::UnsupportedFilter((*name).into()));
        }
    }
    for name in FILTERS {
        let prefix = format!("{name}:");
        if lower.starts_with(&prefix) {
            let value = &word[prefix.len()..];
            return filter_term(name, value);
        }
    }
    // 路径分段。
    if word.contains('/') {
        return path_term(word);
    }
    // 名字词（含通配符时为整名通配符匹配）。
    let wildcards = word.contains('*') || word.contains('?');
    Ok(Term::Name { pattern: word.to_string(), wildcards })
}

fn filter_term(name: &str, value: &str) -> Result<Term, ParseError> {
    match name {
        "ext" => {
            let exts: Vec<String> = value
                .split(';')
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            if exts.is_empty() {
                return Err(ParseError::Syntax("ext: 需要至少一个扩展名，如 ext:pdf;txt".into()));
            }
            Ok(Term::Ext(exts))
        }
        "type" => {
            let group = value.trim().to_ascii_lowercase();
            if super::types::extensions_for(&group).is_none() {
                return Err(ParseError::UnknownTypeGroup(value.trim().into()));
            }
            Ok(Term::Type(group))
        }
        "file" => Ok(Term::File),
        "folder" => Ok(Term::Folder),
        "size" => parse_size(value).map(Term::Size),
        "dm" => parse_date(value).map(Term::DateModified),
        "dc" => parse_date(value).map(Term::DateCreated),
        "infolder" => Ok(Term::InFolder(value.trim_matches('"').to_string())),
        "parent" => Ok(Term::Parent(value.trim_matches('"').to_string())),
        "regex" => {
            regex::Regex::new(value).map_err(|err| ParseError::BadRegex(err.to_string()))?;
            Ok(Term::Regex(value.to_string()))
        }
        _ => unreachable!(),
    }
}

fn path_term(word: &str) -> Result<Term, ParseError> {
    let segments: Vec<PathSegment> = word
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| PathSegment {
            pattern: s.to_string(),
            wildcards: s.contains('*') || s.contains('?'),
        })
        .collect();
    if segments.is_empty() {
        return Err(ParseError::Syntax("路径词为空".into()));
    }
    Ok(Term::Path(segments))
}

// ---- size ----

/// 解析 `>1MB` / `<10KB` / `100KB-5MB` / `1MB`（§7.8：单位固定 1024 进制）。
pub(crate) fn parse_size(value: &str) -> Result<SizeFilter, ParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ParseError::BadSize("(空)".into()));
    }
    if let Some(rest) = value.strip_prefix('>') {
        return Ok(SizeFilter::Gt(parse_size_value(rest)?));
    }
    if let Some(rest) = value.strip_prefix('<') {
        return Ok(SizeFilter::Lt(parse_size_value(rest)?));
    }
    // 区间：第一个不在开头的 '-'。
    if let Some(pos) = value[1..].find('-').map(|p| p + 1) {
        let (a, b) = value.split_at(pos);
        let b = &b[1..];
        return Ok(SizeFilter::Range(parse_size_value(a)?, parse_size_value(b)?));
    }
    Ok(SizeFilter::Exact(parse_size_value(value)?))
}

fn parse_size_value(text: &str) -> Result<u64, ParseError> {
    let text = text.trim();
    let unit_start = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(unit_start);
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| ParseError::BadSize(text.into()))?;
    let multiplier = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1.0,
        "K" | "KB" => 1024.0,
        "M" | "MB" => 1024.0 * 1024.0,
        "G" | "GB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(ParseError::BadSize(text.into())),
    };
    let bytes = number * multiplier;
    if bytes < 0.0 || bytes > u64::MAX as f64 {
        return Err(ParseError::BadSize(text.into()));
    }
    Ok(bytes as u64)
}

// ---- 日期 ----

/// 解析日期关键词 / ISO 日期 / `a..b` 区间（本地时区边界，§7.8）。
pub(crate) fn parse_date(value: &str) -> Result<DateFilter, ParseError> {
    let value = value.trim();
    if let Some((a, b)) = value.split_once("..") {
        let a = resolve_date_bound(a, false)?;
        let b = resolve_date_bound(b, true)?;
        return Ok(DateFilter { start: a, end: b });
    }
    let start = resolve_date_bound(value, false)?;
    let end = resolve_date_bound(value, true)?;
    Ok(DateFilter { start, end })
}

/// 把关键词或 ISO 日期解析成时间戳；`end_of_day` 为 true 时取当天/区间的结束边界。
fn resolve_date_bound(value: &str, end_of_day: bool) -> Result<i64, ParseError> {
    let now = Local::now();
    let today = now.date_naive();
    let bounds = |date: NaiveDate| -> (i64, i64) {
        let start = local_timestamp(date, 0, 0, 0);
        // 排他边界 = 次日零点（不能用 23:59:60：`and_hms_opt` 不支持闰秒）。
        let next = date.succ_opt().unwrap_or(date);
        let end = local_timestamp(next, 0, 0, 0);
        (start, end)
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "today" => {
            let (s, e) = bounds(today);
            Ok(if end_of_day { e } else { s })
        }
        "yesterday" => {
            let (s, e) = bounds(today.pred_opt().unwrap_or(today));
            Ok(if end_of_day { e } else { s })
        }
        "thisweek" => {
            let days_since_monday = now.weekday().num_days_from_monday();
            let monday = today - chrono::Duration::days(i64::from(days_since_monday));
            let (s, e) = bounds(monday);
            Ok(if end_of_day { e } else { s })
        }
        "thismonth" => {
            let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
            let (s, e) = bounds(first);
            Ok(if end_of_day { e } else { s })
        }
        "thisyear" => {
            let first = NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today);
            let (s, e) = bounds(first);
            Ok(if end_of_day { e } else { s })
        }
        "pastweek" => Ok(if end_of_day {
            now.timestamp() + 1
        } else {
            now.timestamp() - 7 * 24 * 3600
        }),
        "pastmonth" => Ok(if end_of_day {
            now.timestamp() + 1
        } else {
            now.timestamp() - 30 * 24 * 3600
        }),
        "pastyear" => Ok(if end_of_day {
            now.timestamp() + 1
        } else {
            now.timestamp() - 365 * 24 * 3600
        }),
        _ => {
            let date = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                .map_err(|_| ParseError::BadDate(value.into()))?;
            let (s, e) = bounds(date);
            Ok(if end_of_day { e } else { s })
        }
    }
}

/// 本地时区的某天某时刻的时间戳（DST 空洞取较早的解）。
fn local_timestamp(date: NaiveDate, hour: u32, min: u32, sec: u32) -> i64 {
    let naive = date
        .and_hms_opt(hour, min, sec)
        .unwrap_or_else(|| date.and_hms_opt(0, 0, 0).unwrap());
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.timestamp(),
        chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        chrono::LocalResult::None => {
            // DST 空洞：用前一小时。
            let earlier = naive - chrono::Duration::hours(1);
            Local
                .from_local_datetime(&earlier)
                .single()
                .map(|dt| dt.timestamp() + 3600)
                .unwrap_or_else(|| naive.and_utc().timestamp())
        }
    }
}

/// 匹配文本归一化（解析阶段不给模式做归一化；由求值侧按大小写设置准备）。
#[allow(dead_code)]
fn normalize_pattern(pattern: &str, case_sensitive: bool) -> String {
    match_text_of(pattern, case_sensitive)
}
