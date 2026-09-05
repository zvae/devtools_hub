use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

/// 所有数据库迁移脚本在编译期嵌入，运行时按版本顺序执行。
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_initial",
        include_str!("../../../migrations/0001_initial.sql"),
    ),
    (
        "0002_tool_history",
        include_str!("../../../migrations/0002_tool_history.sql"),
    ),
];

/// SQLite 存储入口。Connection 通过 Mutex 保护，方便被后台任务和 UI 回调共享。
#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

/// 剪贴板记录的应用层模型，source_app 用于展示复制来源窗口。
#[derive(Clone, Debug)]
pub struct ClipboardRecord {
    pub id: i64,
    pub content: String,
    pub content_type: String,
    pub source_app: Option<String>,
    pub sensitive: bool,
    pub created_at: i64,
}

/// 工具执行历史。输入和输出按工具独立保存，供工具窗口恢复使用。
#[derive(Clone, Debug)]
pub struct ToolHistoryRecord {
    pub id: i64,
    pub input: String,
    pub output: String,
    pub created_at: i64,
}

impl Storage {
    /// 打开平台默认数据目录下的数据库。
    pub fn open_default() -> Result<Self> {
        let data_dir = data_dir()?;
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
        Self::open(data_dir.join("devtools.db"))
    }

    /// 打开指定路径数据库，并立即执行迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref()).with_context(|| {
            format!("failed to open sqlite database {}", path.as_ref().display())
        })?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;
        Ok(storage)
    }

    /// 执行未应用的 SQL 迁移，已执行版本记录在 schema_migrations 中。
    pub fn migrate(&self) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );",
        )?;

        for (version, sql) in MIGRATIONS {
            let applied = tx
                .query_row(
                    "SELECT 1 FROM schema_migrations WHERE version = ?1",
                    [version],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();

            if !applied {
                tx.execute_batch(sql)?;
                tx.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                    params![version, now_unix()],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// 兼容旧调用：未提供来源应用时按普通剪贴板文本保存。
    pub fn upsert_clipboard_text(&self, content: &str) -> Result<Option<ClipboardRecord>> {
        self.upsert_clipboard_text_from(content, None)
    }

    /// 保存剪贴板文本。使用内容哈希去重，重复复制时只刷新更新时间和来源应用。
    pub fn upsert_clipboard_text_from(
        &self,
        content: &str,
        source_app: Option<&str>,
    ) -> Result<Option<ClipboardRecord>> {
        let content = content.trim_matches('\0').trim();
        if content.is_empty() {
            return Ok(None);
        }

        let hash = hash_text(content);
        let content_type = detect_content_type(content);
        let sensitive = is_sensitive(content);
        let now = now_unix();
        let conn = self.lock()?;

        conn.execute(
            "INSERT INTO clipboard_item (
                content, content_type, hash, source_app, sensitive, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(hash) DO UPDATE SET
                source_app = COALESCE(excluded.source_app, clipboard_item.source_app),
                updated_at = excluded.updated_at",
            params![
                content,
                content_type,
                hash,
                source_app,
                sensitive as i32,
                now
            ],
        )?;

        let id = conn.query_row(
            "SELECT id FROM clipboard_item WHERE hash = ?1",
            [hash],
            |row| row.get::<_, i64>(0),
        )?;

        Ok(Some(ClipboardRecord {
            id,
            content: content.to_string(),
            content_type: content_type.to_string(),
            source_app: source_app.map(str::to_string),
            sensitive,
            created_at: now,
        }))
    }

    /// 获取最近更新的剪贴板记录，用于空查询时展示历史列表。
    pub fn latest_clipboard(&self, limit: usize) -> Result<Vec<ClipboardRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, content, content_type, source_app, sensitive, created_at
             FROM clipboard_item
             ORDER BY updated_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query(params![limit as i64])?;
        let records = rows_to_clipboard(rows)?;
        Ok(records)
    }

    /// 使用 SQLite FTS5 搜索剪贴板内容。
    pub fn search_clipboard(&self, query: &str, limit: usize) -> Result<Vec<ClipboardRecord>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.latest_clipboard(limit);
        }

        let conn = self.lock()?;
        let fts_query = build_fts_query(trimmed);
        let mut stmt = conn.prepare(
            "SELECT item.id, item.content, item.content_type, item.source_app, item.sensitive, item.created_at
             FROM clipboard_item_fts fts
             JOIN clipboard_item item ON item.id = fts.rowid
             WHERE clipboard_item_fts MATCH ?1
             ORDER BY bm25(clipboard_item_fts), item.updated_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query(params![fts_query, limit as i64])?;
        let records = rows_to_clipboard(rows)?;
        Ok(records)
    }

    /// 按 ID 读取剪贴板记录，点击历史项复制回剪贴板时使用。
    pub fn clipboard_by_id(&self, id: i64) -> Result<Option<ClipboardRecord>> {
        let conn = self.lock()?;
        let record = conn
            .query_row(
                "SELECT id, content, content_type, source_app, sensitive, created_at
                 FROM clipboard_item
                 WHERE id = ?1",
                [id],
                |row| {
                    Ok(ClipboardRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        content_type: row.get(2)?,
                        source_app: row.get(3)?,
                        sensitive: row.get::<_, i32>(4)? != 0,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()?;

        Ok(record)
    }

    /// 清空剪贴板历史，不影响系统当前剪贴板内容。
    pub fn clear_clipboard(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM clipboard_item", [])?;
        Ok(())
    }

    /// 读取键值配置。
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock()?;
        let value = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(value)
    }

    /// 写入键值配置，已存在时更新时间和值。
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings(key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![key, value, now_unix()],
        )?;
        Ok(())
    }

    /// 记录命令执行历史，输入只存哈希，避免保留敏感原文。
    pub fn record_execution(&self, command_id: &str, input_hash: Option<&str>) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO execution_history(command_id, input_hash, executed_at)
             VALUES (?1, ?2, ?3)",
            params![command_id, input_hash, now_unix()],
        )?;
        Ok(())
    }

    /// 保存一次工具输入和输出，并按每工具上限删除最早的历史记录。
    pub fn append_tool_history(
        &self,
        tool_id: &str,
        input: &str,
        output: &str,
        limit: usize,
    ) -> Result<()> {
        let input = input.trim();
        if limit == 0 || input.is_empty() {
            return Ok(());
        }
        let conn = self.lock()?;
        // 同一工具的相同输入只保留最新一次，避免自动计算产生重复历史。
        conn.execute(
            "DELETE FROM tool_history WHERE tool_id = ?1 AND input = ?2",
            params![tool_id, input],
        )?;
        conn.execute(
            "INSERT INTO tool_history(tool_id, input, output, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![tool_id, input, output, now_unix()],
        )?;
        trim_tool_history(&conn, tool_id, limit)?;
        Ok(())
    }

    /// 返回指定工具的最新历史，供工具窗口展示和恢复。
    pub fn tool_history(&self, tool_id: &str, limit: usize) -> Result<Vec<ToolHistoryRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, input, output, created_at
             FROM tool_history
             WHERE tool_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![tool_id, limit as i64], |row| {
            Ok(ToolHistoryRecord {
                id: row.get(0)?,
                input: row.get(1)?,
                output: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// 写入每工具历史上限，并立即裁剪现有的每个工具历史。
    pub fn set_tool_history_limit(&self, limit: usize) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings(key, value, updated_at)
             VALUES ('tool_history_limit', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![limit.to_string(), now_unix()],
        )?;
        let mut stmt = conn.prepare("SELECT DISTINCT tool_id FROM tool_history")?;
        let tool_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for tool_id in tool_ids {
            trim_tool_history(&conn, &tool_id, limit)?;
        }
        Ok(())
    }

    /// 获取 SQLite 连接锁，统一处理 mutex poisoned 错误。
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("sqlite connection mutex was poisoned"))
    }
}

/// 删除单个工具超出保留数量的最早记录。limit 为 0 时清空该工具全部历史。
fn trim_tool_history(conn: &Connection, tool_id: &str, limit: usize) -> Result<()> {
    if limit == 0 {
        conn.execute("DELETE FROM tool_history WHERE tool_id = ?1", [tool_id])?;
        return Ok(());
    }
    conn.execute(
        "DELETE FROM tool_history
         WHERE id IN (
             SELECT id FROM tool_history
             WHERE tool_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT -1 OFFSET ?2
         )",
        params![tool_id, limit as i64],
    )?;
    Ok(())
}

/// 将 rusqlite 行游标转换成剪贴板模型列表。
fn rows_to_clipboard(mut rows: rusqlite::Rows<'_>) -> Result<Vec<ClipboardRecord>> {
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(ClipboardRecord {
            id: row.get(0)?,
            content: row.get(1)?,
            content_type: row.get(2)?,
            source_app: row.get(3)?,
            sensitive: row.get::<_, i32>(4)? != 0,
            created_at: row.get(5)?,
        });
    }
    Ok(records)
}

/// 解析系统应用数据目录。
pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "DevToolsHub")
        .context("failed to resolve platform application data directory")?;
    Ok(dirs.data_dir().to_path_buf())
}

/// 计算内容哈希，用于剪贴板去重和执行历史输入摘要。
pub fn hash_text(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 返回 UTC Unix 秒时间戳。
fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// 基于文本特征粗略判断内容类型，用于后续工具推荐和列表筛选。
fn detect_content_type(content: &str) -> &'static str {
    let trimmed = content.trim();
    if serde_json_like(trimmed) {
        "json"
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        "url"
    } else if trimmed.contains("SELECT ") || trimmed.contains("select ") {
        "sql"
    } else if trimmed.split('.').count() == 3 && trimmed.starts_with("eyJ") {
        "jwt"
    } else {
        "text"
    }
}

/// 快速判断文本外形是否像 JSON，避免每次都完整解析。
fn serde_json_like(value: &str) -> bool {
    (value.starts_with('{') && value.ends_with('}'))
        || (value.starts_with('[') && value.ends_with(']'))
}

/// 识别常见敏感内容形态，列表展示时会自动打码。
fn is_sensitive(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    content.starts_with("ghp_")
        || content.starts_with("github_pat_")
        || content.starts_with("AKIA")
        || lower.contains("bearer ")
        || lower.contains("private key")
        || lower.contains("password=")
        || lower.contains("postgres://")
        || lower.contains("mysql://")
}

/// 将普通查询词转换为 FTS5 前缀查询表达式。
fn build_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| {
            let escaped = term.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时数据库应能自动迁移，并支持剪贴板全文搜索。
    #[test]
    fn migrates_and_searches_clipboard_text() {
        let db_path = std::env::temp_dir().join(format!(
            "devtools-hub-storage-test-{}-{}.db",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));

        let storage = Storage::open(&db_path).expect("storage should open");
        storage
            .upsert_clipboard_text("hello from clipboard")
            .expect("clipboard insert should work");

        let results = storage
            .search_clipboard("hello", 5)
            .expect("clipboard search should work");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello from clipboard");

        drop(storage);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    /// 设置表应支持写入后再次读取。
    #[test]
    fn stores_settings() {
        let db_path = std::env::temp_dir().join(format!(
            "devtools-hub-settings-test-{}-{}.db",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));

        let storage = Storage::open(&db_path).expect("storage should open");
        storage
            .set_setting("theme", "light")
            .expect("setting should save");

        assert_eq!(
            storage.get_setting("theme").expect("setting should load"),
            Some("light".into())
        );

        drop(storage);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    #[test]
    fn keeps_history_per_tool_with_a_configured_limit() {
        let db_path = std::env::temp_dir().join(format!(
            "devtools-hub-history-test-{}-{}.db",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        storage
            .append_tool_history("tool.base64.encode", "a", "YQ==", 2)
            .expect("first history should save");
        storage
            .append_tool_history("tool.base64.encode", "b", "Yg==", 2)
            .expect("second history should save");
        storage
            .append_tool_history("tool.base64.encode", "c", "Yw==", 2)
            .expect("third history should trim the oldest row");

        let history = storage
            .tool_history("tool.base64.encode", 20)
            .expect("history should load");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].input, "c");
        assert_eq!(history[1].input, "b");

        storage
            .set_tool_history_limit(1)
            .expect("history limit should trim existing records");
        assert_eq!(
            storage
                .tool_history("tool.base64.encode", 20)
                .expect("trimmed history should load")
                .len(),
            1
        );

        drop(storage);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }

    #[test]
    fn ignores_empty_and_deduplicates_tool_history_input() {
        let db_path = std::env::temp_dir().join(format!(
            "devtools-hub-history-dedupe-test-{}-{}.db",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let storage = Storage::open(&db_path).expect("storage should open");
        storage
            .append_tool_history("tool.base64.encode", "", "", 20)
            .expect("empty history should be ignored");
        storage
            .append_tool_history("tool.base64.encode", "hello", "aGVsbG8=", 20)
            .expect("first history should save");
        storage
            .append_tool_history("tool.base64.encode", "hello", "updated", 20)
            .expect("duplicate input should replace the existing record");

        let history = storage
            .tool_history("tool.base64.encode", 20)
            .expect("history should load");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].input, "hello");
        assert_eq!(history[0].output, "updated");

        drop(storage);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    }
}
