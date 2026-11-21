//! 外部载荷生命周期。调用方持有 SQLite IMMEDIATE 事务，写入与回收互斥。

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use rusqlite::Connection;

use crate::storage::sync_directory;

/// 事务失败时自动移除新载荷；提交后将所有权交给数据库。
pub(super) struct PendingBlob(Option<tempfile::TempPath>);

impl PendingBlob {
    pub fn write(dir: &Path, bytes: &[u8]) -> Result<Self> {
        let mut file = tempfile::Builder::new()
            .prefix("payload-")
            .suffix(".zst")
            .tempfile_in(dir)?;
        file.write_all(bytes).context("写入外部数据失败")?;
        file.as_file().sync_all().context("同步外部数据失败")?;
        sync_directory(dir)?;
        Ok(Self(Some(file.into_temp_path())))
    }

    pub fn name(&self) -> Option<String> {
        self.0
            .as_ref()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
    }

    pub fn committed(mut self) {
        if let Some(path) = self.0.take() {
            // keep 的错误也携带 TempPath；不能让析构删除已经被数据库引用的文件。
            if let Err(mut error) = path.keep() {
                tracing::error!("保留已提交外部数据失败：{}", error.error);
                error.path.disable_cleanup(true);
            }
        }
    }
}

pub(super) fn path(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none()
            && !name.contains(['/', '\\'])
            && !name.contains(".."),
        "无效的外部数据文件名"
    );
    Ok(dir.join(name))
}

/// 只回收本库的载荷。事务锁保证不会删除另一个连接尚未提交的新文件。
pub(super) fn collect_orphans(conn: &Connection, dir: &Path) -> Result<()> {
    let mut statement =
        conn.prepare("SELECT external_file FROM payloads WHERE external_file IS NOT NULL")?;
    let referenced = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if entry.path().extension().is_some_and(|ext| ext == "zst")
            && !referenced.contains(name.to_string_lossy().as_ref())
        {
            if let Err(error) = fs::remove_file(entry.path()) {
                tracing::warn!("回收孤立载荷 {} 失败：{error}", entry.path().display());
            }
        }
    }
    Ok(())
}
