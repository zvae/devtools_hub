//! 读取其他应用中选中的内容。
//!
//! 与 uTools 的做法相同：记下剪贴板的变化序号（macOS 的 `changeCount`、Windows 的
//! `GetClipboardSequenceNumber`），向前台应用模拟一次复制（⌘C / Ctrl+C），
//! 序号变了说明有选中内容，从剪贴板读出来；没变说明没有选中任何内容。
//! uTools 读完后把选中内容留在剪贴板里（只是暂停 300 ms 剪贴板记录）；
//! 这里读完后把剪贴板恢复成原来的内容，这次复制也不会记进剪贴板历史。

use std::time::{Duration, Instant};

use devtools_clipboard::{uti, Snapshot};

use crate::platform::pasteboard::Pasteboard;
use crate::shared::Shared;

/// 等待前台应用响应复制的最长时间。uTools 固定等 50 ms，
/// 浏览器、Electron 应用偶尔更慢，这里轮询到有变化为止；没有选中内容时要等满这段时间。
const TIMEOUT: Duration = Duration::from_millis(300);
/// 等待期间检查剪贴板序号的间隔。
const POLL: Duration = Duration::from_millis(10);
/// 序号变化后再等一会儿：有的应用分几次写入剪贴板（先清空再写入各种格式）。
const SETTLE: Duration = Duration::from_millis(30);

/// 选中的内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Text(String),
    /// 在访达 / 资源管理器中选中的文件。
    Files(Vec<String>),
}

impl Selection {
    /// 从复制得到的剪贴板内容中取出选中的文件或文字；都没有时返回 `None`。
    fn from_snapshot(snapshot: &Snapshot) -> Option<Self> {
        let analysis = uti::analyze(snapshot);
        if !analysis.file_paths.is_empty() {
            return Some(Selection::Files(analysis.file_paths));
        }
        uti::plain_text(snapshot)
            .filter(|text| !text.trim().is_empty())
            .map(Selection::Text)
    }

    /// 面板标题，与 uTools 的写法一致：“选中的文本 389 个”。
    pub fn label(&self) -> String {
        match self {
            Selection::Text(text) => format!("选中的文本 {} 个", text.chars().count()),
            Selection::Files(paths) => format!("选中的文件 {} 个", paths.len()),
        }
    }
}

/// 模拟复制并读出选中的内容，之后恢复剪贴板。`send_copy` 负责发送复制快捷键。
///
/// 会阻塞调用线程直到读到内容或超时（最多约 0.3 秒）；期间剪贴板监听线程暂停。
pub fn capture(
    shared: &Shared,
    pasteboard: &Pasteboard,
    send_copy: impl FnOnce(),
) -> Option<Selection> {
    let mut selection = None;
    shared.write_pasteboard(|| {
        let before = pasteboard.change_count();
        let saved = pasteboard.read();
        send_copy();
        if !wait_for_change(pasteboard, before) {
            tracing::debug!("模拟复制后剪贴板没有变化：没有选中内容");
            return before;
        }
        selection = Selection::from_snapshot(&pasteboard.read());
        pasteboard.write(&saved)
    });
    selection
}

/// 等待剪贴板序号变化并稳定下来。
fn wait_for_change(pasteboard: &Pasteboard, before: isize) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    let mut changed: Option<(isize, Instant)> = None;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        let count = pasteboard.change_count();
        if count == before {
            continue;
        }
        match changed {
            Some((last, at)) if last == count => {
                if at.elapsed() >= SETTLE {
                    return true;
                }
            }
            _ => changed = Some((count, Instant::now())),
        }
    }
    changed.is_some()
}

// 在私有命名剪贴板上验证（不影响系统剪贴板）；Windows 没有命名剪贴板。
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use devtools_clipboard::Representation;
    use std::sync::atomic::Ordering;

    #[test]
    fn captures_selection_and_restores_clipboard() {
        let shared = Shared::new(&Settings::default());
        let pasteboard = Pasteboard::unique();
        let original = Snapshot {
            items: vec![vec![
                Representation::new(uti::PLAIN_TEXT, "原来的内容".as_bytes()),
                Representation::new(uti::HTML, b"<i>x</i>".to_vec()),
            ]],
        };
        pasteboard.write(&original);

        // 模拟前台应用响应复制：把选中的文本写进剪贴板。
        let selection = capture(&shared, &pasteboard, || {
            pasteboard.write(&Snapshot::from_text("{\"a\": 1}"));
        });
        assert_eq!(selection, Some(Selection::Text("{\"a\": 1}".into())));
        assert_eq!(pasteboard.read(), original, "读完后应恢复原来的内容");
        // 恢复产生的变化由本应用写入，监听线程应跳过。
        assert_eq!(
            shared.ignore_change_count.load(Ordering::SeqCst),
            pasteboard.change_count()
        );
        assert!(!shared.writing.load(Ordering::SeqCst));

        // 没有选中内容：剪贴板不变，也不会被改写。
        let count = pasteboard.change_count();
        assert_eq!(capture(&shared, &pasteboard, || {}), None);
        assert_eq!(pasteboard.change_count(), count);
        pasteboard.release();
    }

    #[test]
    fn labels_like_utools() {
        assert_eq!(
            Selection::Text("预算和投入".into()).label(),
            "选中的文本 5 个"
        );
        assert_eq!(
            Selection::Files(vec!["/a".into(), "/b".into()]).label(),
            "选中的文件 2 个"
        );
    }
}
