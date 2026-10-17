//! 从富文本（RTF / HTML）里取出代码的高亮样式。
//!
//! IDE 复制代码时会同时写入带语法高亮的富文本：JetBrains 系列写 RTF（也写 HTML），
//! VS Code 写 HTML，Xcode、终端写 RTF。PasteNow 按这些样式显示代码，从浏览器复制的同一段文字
//! 则按普通文本显示。这里也只在文字基本都是等宽字体（也就是代码）时返回样式，
//! 网页、文档里的富文本仍按纯文本显示，免得把网页的配色搬进列表。
//!
//! 只解析显示需要的部分：文字、颜色、粗体、斜体、字体是否等宽。
//!
//! RTF 与 HTML 的解析分别在 `rtf`、`html` 中，两者把文字和样式送进同一个 [`Output`]。

mod html;
mod rtf;

use crate::model::Snapshot;
use crate::uti;
use html::html_runs;
use rtf::rtf_runs;

/// 一段样式相同的文字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledRun {
    pub text: String,
    /// 没有指定颜色时为 `None`（用界面的正文颜色）。
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
}

/// 等宽字体的文字至少占这么多，才当作代码。
const MONOSPACE_SHARE: f32 = 0.8;

/// 快照里的代码样式：优先 RTF，其次 HTML；不是代码时返回 `None`。最多取 `max_chars` 个字符。
pub fn code_runs(snapshot: &Snapshot, max_chars: usize) -> Option<Vec<StyledRun>> {
    if let Some(rtf) = snapshot.first(uti::RTF) {
        return rtf_runs(rtf, max_chars);
    }
    let html = snapshot.first(uti::HTML)?;
    html_runs(&String::from_utf8_lossy(html), max_chars)
}

/// 收集输出：合并样式相同的相邻文字，统计等宽字体的比例。
#[derive(Default)]
struct Output {
    runs: Vec<StyledRun>,
    /// 已输出的字符数，到 `max_chars` 就停止。
    chars: usize,
    max_chars: usize,
    /// 非空白字符数，以及其中等宽字体的字符数（判断是不是代码）。
    visible: usize,
    monospace: usize,
}

/// 解析过程中当前位置的样式。
#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    color: Option<[u8; 3]>,
    bold: bool,
    italic: bool,
    monospace: bool,
}

impl Output {
    fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Self::default()
        }
    }

    /// 已经取够 `max_chars` 个字符。
    fn full(&self) -> bool {
        self.chars >= self.max_chars
    }

    /// 追加文字：与上一段样式相同时合并（等宽与否不影响显示，不参与比较）。
    fn push(&mut self, text: &str, style: Style) {
        for ch in text.chars() {
            if self.full() {
                return;
            }
            self.chars += 1;
            if !ch.is_whitespace() {
                self.visible += 1;
                if style.monospace {
                    self.monospace += 1;
                }
            }
            match self.runs.last_mut() {
                Some(run)
                    if run.color == style.color
                        && run.bold == style.bold
                        && run.italic == style.italic =>
                {
                    run.text.push(ch);
                }
                _ => self.runs.push(StyledRun {
                    text: ch.to_string(),
                    color: style.color,
                    bold: style.bold,
                    italic: style.italic,
                }),
            }
        }
    }

    /// 输出为空或以换行结尾（块级元素前后只补一个换行）。
    fn ends_with_newline(&self) -> bool {
        self.runs.last().is_none_or(|run| run.text.ends_with('\n'))
    }

    /// 去掉末尾的换行与空白；不是代码时返回 `None`。
    fn finish(mut self) -> Option<Vec<StyledRun>> {
        if self.visible == 0 || (self.monospace as f32) < self.visible as f32 * MONOSPACE_SHARE {
            return None;
        }
        while let Some(run) = self.runs.last_mut() {
            let trimmed = run.text.trim_end().len();
            run.text.truncate(trimmed);
            if !run.text.is_empty() {
                break;
            }
            self.runs.pop();
        }
        Some(self.runs)
    }
}

/// 常见的等宽字体（RTF 字体表里的 `\fmodern` 也算）。
fn is_monospace_font(name: &str) -> bool {
    const NAMES: [&str; 16] = [
        "mono",
        "menlo",
        "monaco",
        "courier",
        "consolas",
        "fira code",
        "firacode",
        "source code",
        "sourcecode",
        "cascadia",
        "hack",
        "inconsolata",
        "iosevka",
        "sf mono",
        "sfmono",
        "lucida console",
    ];
    let name = name.to_ascii_lowercase();
    NAMES.iter().any(|mono| name.contains(mono))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(runs: &[StyledRun]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn reads_jetbrains_rtf() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0\fnil JetBrains Mono;}}
{\colortbl;\red169\green183\blue198;\red43\green43\blue43;\red204\green120\blue50;\red152\green118\blue170;}
\f0\fs26\cb2\cf1\highlight2 \cf3 SELECT\cf1  r.* \cf3 FROM\cf1  t_order r\par
\cf3 WHERE\cf1  r.\cf4 id\cf1  = 1\par
}";
        let runs = rtf_runs(rtf, 1000).expect("等宽字体的 RTF 是代码");
        assert_eq!(plain(&runs), "SELECT r.* FROM t_order r\nWHERE r.id = 1");
        assert_eq!(runs[0].text, "SELECT");
        assert_eq!(runs[0].color, Some([204, 120, 50]));
        assert_eq!(runs[1].color, Some([169, 183, 198]));
        let id = runs.iter().find(|run| run.text == "id").unwrap();
        assert_eq!(id.color, Some([152, 118, 170]));
    }

    #[test]
    fn reads_cocoa_rtf_with_unicode_and_skips_destinations() {
        let rtf = br"{\rtf1\ansi\ansicpg936\cocoartf2822
\cocoatextscaling0\cocoaplatform0{\fonttbl\f0\fnil\fcharset0 Menlo-Bold;\f1\fnil\fcharset0 Menlo-Regular;}
{\colortbl;\red255\green255\blue255;\red155\green35\blue147;}
{\*\expandedcolortbl;;\csgenericrgb\c60759\c13753\c57628;}
\pard\tx593\pardeftab593\partightenfactor0

\f0\b\fs24 \cf2 let\f1\b0 \cf0  name = \cf2 \uc0\u20013 \u25991 \cf0 \
}";
        let runs = rtf_runs(rtf, 1000).unwrap();
        assert_eq!(plain(&runs), "let name = 中文");
        assert!(runs[0].bold);
        assert_eq!(runs[0].color, Some([155, 35, 147]));
        assert_eq!(runs[1].color, None, "\\cf0 是自动颜色");
    }

    #[test]
    fn proportional_rtf_is_not_code() {
        let rtf = br"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}{\colortbl;\red255\green0\blue0;}\f0\cf1 Hello world\par}";
        assert_eq!(rtf_runs(rtf, 1000), None);
    }

    #[test]
    fn reads_vscode_html() {
        let html = "<meta charset='utf-8'><div style=\"color: #cccccc;background-color: #1f1f1f;font-family: Menlo, Monaco, 'Courier New', monospace;font-weight: normal;font-size: 12px;line-height: 18px;white-space: pre;\"><div><span style=\"color: #569cd6;\">fn</span><span style=\"color: #cccccc;\"> </span><span style=\"color: #dcdcaa;\">main</span><span style=\"color: #cccccc;\">() {</span></div><div><span style=\"color: #cccccc;\">    </span><span style=\"color: #c586c0;\">let</span><span style=\"color: #cccccc;\"> a = </span><span style=\"color: #ce9178;\">&quot;x &amp; y&quot;</span><span style=\"color: #cccccc;\">;</span></div><div><br></div><div><span style=\"color: #cccccc;\">}</span></div></div>";
        let runs = html_runs(html, 1000).expect("等宽字体的 HTML 是代码");
        assert_eq!(plain(&runs), "fn main() {\n    let a = \"x & y\";\n\n}");
        assert_eq!(runs[0].color, Some([0x56, 0x9c, 0xd6]));
        let string = runs.iter().find(|run| run.text.contains("x & y")).unwrap();
        assert_eq!(string.color, Some([0xce, 0x91, 0x78]));
    }

    #[test]
    fn reads_jetbrains_html() {
        let html = "<html><head><style type=\"text/css\">.s0 { color: #cc7832;}</style></head><body><pre style=\"background-color:#2b2b2b;color:#a9b7c6;font-family:'JetBrains Mono',monospace;font-size:9.8pt;\"><span style=\"color:#cc7832;font-weight:bold;\">SELECT</span> *\n<span style=\"color:#cc7832;\">FROM</span> t</pre></body></html>";
        let runs = html_runs(html, 1000).unwrap();
        assert_eq!(plain(&runs), "SELECT *\nFROM t");
        assert!(runs[0].bold);
        assert_eq!(runs[1].color, Some([0xa9, 0xb7, 0xc6]));
    }

    #[test]
    fn web_html_is_not_code() {
        let html = "<meta charset='utf-8'><span style=\"color: rgb(32, 33, 36); font-family: arial, sans-serif;\">and r.create_time &gt; NOW()</span>";
        assert_eq!(html_runs(html, 1000), None);
        // 网页里的行内代码只占一小部分，也不算。
        let html = "<p style=\"font-family: Helvetica\">运行 <code>cargo build</code> 之后再打开应用，然后检查输出目录</p>";
        assert_eq!(html_runs(html, 1000), None);
    }

    #[test]
    fn stops_at_max_chars() {
        let html = "<pre>abcdefghij</pre>";
        assert_eq!(plain(&html_runs(html, 4).unwrap()), "abcd");
    }
}
