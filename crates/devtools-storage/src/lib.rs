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

const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_initial",
    include_str!("../../../migrations/0001_initial.sql"),
)];

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug)]
pub struct ClipboardRecord {
    pub id: i64,
    pub content: String,
    pub content_type: String,
    pub source_app: Option<String>,
    pub sensitive: bool,
    pub created_at: i64,
}

impl Storage {
    pub fn open_default() -> Result<Self> {
        let data_dir = data_dir()?;
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
        Self::open(data_dir.join("devtools.db"))
    }

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

    pub fn upsert_clipboard_text(&self, content: &str) -> Result<Option<ClipboardRecord>> {
        self.upsert_clipboard_text_from(content, None)
    }

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

    pub fn clear_clipboard(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM clipboard_item", [])?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.lock()?;
        let value = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        Ok(value)
    }

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

    pub fn record_execution(&self, command_id: &str, input_hash: Option<&str>) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO execution_history(command_id, input_hash, executed_at)
             VALUES (?1, ?2, ?3)",
            params![command_id, input_hash, now_unix()],
        )?;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("sqlite connection mutex was poisoned"))
    }
}

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

pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "DevToolsHub")
        .context("failed to resolve platform application data directory")?;
    Ok(dirs.data_dir().to_path_buf())
}

pub fn hash_text(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

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

fn serde_json_like(value: &str) -> bool {
    (value.starts_with('{') && value.ends_with('}'))
        || (value.starts_with('[') && value.ends_with(']'))
}

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
}
