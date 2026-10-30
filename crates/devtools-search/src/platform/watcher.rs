//! 文件系统监听（§8）：macOS 用 FSEvents、Windows 用 `ReadDirectoryChangesW`，
//! 具体后端由 `notify` 按平台选择，core 只认识 [`devtools_search_core::ports::Watcher`] 的语义。
//!
//! 事件在这里合并成"一批路径"再交给索引更新：短时间内的多次写入只触发一次提交，
//! 避免每写一个字节就重建一次派生数据。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// 等待第一批事件的最长时间：没有事件时返回 `None`，让调用方检查取消。
const IDLE_POLL: Duration = Duration::from_millis(200);
/// 一批最多合并的路径数，超过就按"溢出"处理（转全量重扫）。
const MAX_BATCH: usize = 4096;

/// 一批变化。
pub enum WatchBatch {
    /// 变化的绝对路径（可能有重复，调用方去重）。
    Paths(Vec<PathBuf>),
    /// 事件队列溢出或后端报错：索引需要重新全量校验。
    Overflow,
}

/// 一个根的监听器。
pub struct FsWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<notify::Event>>,
}

impl FsWatcher {
    /// 开始递归监听一个目录。失败时返回原因（权限、卷已拔出等）。
    pub fn start(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                let _ = tx.send(result);
            },
            Config::default(),
        )?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self { watcher, rx })
    }

    /// 停止监听（卷拔出 / 重建索引前调用）。
    pub fn stop(&mut self, root: &Path) {
        let _ = self.watcher.unwatch(root);
    }

    /// 取下一批变化：等到第一个事件后，再静默收集 `idle` 时间或攒满一批。
    pub fn next_batch(
        &self,
        idle: Duration,
        cancel: &AtomicBool,
    ) -> Option<WatchBatch> {
        let mut paths = Vec::new();
        match self.rx.recv_timeout(IDLE_POLL) {
            Ok(Ok(event)) => collect(event, &mut paths),
            Ok(Err(err)) => {
                tracing::warn!("文件监听出错：{err}");
                return Some(WatchBatch::Overflow);
            }
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => return Some(WatchBatch::Overflow),
        }
        let deadline = Instant::now() + idle;
        while !cancel.load(Ordering::Relaxed) && Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(event)) => {
                    collect(event, &mut paths);
                    if paths.len() > MAX_BATCH {
                        return Some(WatchBatch::Overflow);
                    }
                }
                Ok(Err(err)) => {
                    tracing::warn!("文件监听出错：{err}");
                    return Some(WatchBatch::Overflow);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Some(WatchBatch::Overflow),
            }
        }
        if paths.is_empty() {
            None
        } else {
            Some(WatchBatch::Paths(paths))
        }
    }
}

/// 只关心"内容/结构变化"，忽略纯访问事件（读取文件不应该触发重建）。
fn collect(event: notify::Event, out: &mut Vec<PathBuf>) {
    match event.kind {
        EventKind::Access(_) => {}
        _ => out.extend(event.paths),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实文件系统上的监听：建文件后应收到包含该路径的一批事件。
    #[test]
    #[ignore = "需要真实文件系统事件（CI 上偶发不稳定），手动运行"]
    fn reports_created_file() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = FsWatcher::start(dir.path()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let target = dir.path().join("watched.txt");
        std::fs::write(&target, "hello").unwrap();
        let cancel = AtomicBool::new(false);
        let batch = watcher.next_batch(Duration::from_millis(300), &cancel);
        let Some(WatchBatch::Paths(paths)) = batch else {
            panic!("应收到创建事件");
        };
        assert!(
            paths.iter().any(|path| path.ends_with("watched.txt")),
            "路径应包含 watched.txt：{paths:?}"
        );
    }
}
