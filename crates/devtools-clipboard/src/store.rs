//! SQLite 存储。
//!
//! 结构参考 PasteNow 的 Core Data 模型：
//! - `items` ≈ `ZPASTEITEM`：元数据、指纹、来源应用、缩略图；
//! - `payloads` ≈ `ZPASTEITEMDATA`：压缩后的完整快照，大对象存到外部文件
//!   （Core Data 的 “Allows External Storage” 会把大 BLOB 放进 `_EXTERNAL_DATA/` 目录）；
//! - `lists` / `list_items` ≈ `ZPASTELIST`：用户列表，列表中的条目不参与自动清理。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};

use crate::codec;
use crate::model::{Capture, ClipItem, ClipList, ContentKind, Snapshot, SourceApp};
use crate::thumbnail::Thumbnail;
use crate::uti::Analysis;

/// 数据库文件名与外部数据目录名（都在数据目录下）。
const DB_FILE: &str = "history.sqlite3";
const BLOB_DIR: &str = "blobs";
/// 压缩后超过该大小的数据写到外部文件，保持数据库文件精简。
const EXTERNAL_THRESHOLD: usize = 256 * 1024;
/// 数据库结构的版本，保存在 `user_version` 里，结构变化时递增并在 `migrate` 中升级。
const SCHEMA_VERSION: i64 = 1;

/// 内置的“收藏”列表 id。
pub const FAVORITES_LIST_ID: i64 = 1;

/// 当前时间（Unix 毫秒）。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// [`Store::record`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordOutcome {
    pub id: i64,
    /// false 表示命中已有指纹，只更新了时间和复制次数。
    pub inserted: bool,
}

/// 列表查询条件。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemQuery {
    /// 空格分隔的多个关键字，全部命中才返回。
    pub text: String,
    pub kind: Option<KindFilter>,
    /// 只返回该列表中的条目。
    pub list_id: Option<i64>,
    /// 最多返回的条数，0 表示默认的 500 条。
    pub limit: usize,
}

/// 按内容类型筛选。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindFilter {
    /// 纯文本 + 富文本。
    Textual,
    Exact(ContentKind),
}

/// 设置页“数据”分组显示的统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub item_count: i64,
    pub database_bytes: u64,
    pub external_bytes: u64,
}

/// 剪贴板历史数据库。每个线程各自打开一个，不跨线程共享连接。
pub struct Store {
    conn: Connection,
    dir: PathBuf,
    blob_dir: PathBuf,
}

impl Store {
    /// 打开（或创建）数据目录下的数据库。多个线程可以各自打开一个 `Store`，WAL 模式下读写互不阻塞。
    pub fn open(dir: &Path) -> Result<Self> {
        let blob_dir = dir.join(BLOB_DIR);
        fs::create_dir_all(&blob_dir)
            .with_context(|| format!("创建目录失败：{}", blob_dir.display()))?;
        let conn = Connection::open(dir.join(DB_FILE)).context("打开数据库失败")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        enable_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self {
            conn,
            dir: dir.to_path_buf(),
            blob_dir,
        };
        store.migrate()?;
        Ok(store)
    }

    /// 建表并创建内置的“收藏”列表（按 `user_version` 判断是否需要升级）。
    fn migrate(&mut self) -> Result<()> {
        // IMMEDIATE 事务：多个连接同时打开时串行执行迁移，而不是在升级写锁时直接报 busy。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS items (
                     id INTEGER PRIMARY KEY,
                     checksum TEXT NOT NULL UNIQUE,
                     kind INTEGER NOT NULL,
                     title TEXT NOT NULL,
                     search_text TEXT NOT NULL,
                     source_bundle_id TEXT,
                     source_name TEXT,
                     created_at INTEGER NOT NULL,
                     last_used_at INTEGER NOT NULL,
                     copy_count INTEGER NOT NULL DEFAULT 1,
                     paste_count INTEGER NOT NULL DEFAULT 0,
                     byte_size INTEGER NOT NULL,
                     file_count INTEGER NOT NULL DEFAULT 0,
                     image_width INTEGER,
                     image_height INTEGER,
                     thumbnail BLOB
                 );
                 CREATE INDEX IF NOT EXISTS items_last_used ON items(last_used_at DESC);
                 CREATE INDEX IF NOT EXISTS items_kind ON items(kind, last_used_at DESC);
                 CREATE TABLE IF NOT EXISTS payloads (
                     item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
                     inline_data BLOB,
                     external_file TEXT
                 );
                 CREATE TABLE IF NOT EXISTS lists (
                     id INTEGER PRIMARY KEY,
                     name TEXT NOT NULL UNIQUE,
                     created_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS list_items (
                     list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
                     item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                     added_at INTEGER NOT NULL,
                     PRIMARY KEY (list_id, item_id)
                 );
                 CREATE INDEX IF NOT EXISTS list_items_item ON list_items(item_id);",
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO lists (id, name, created_at) VALUES (?1, '收藏', ?2)",
                params![FAVORITES_LIST_ID, now_ms()],
            )?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 记录一次复制：指纹已存在则更新时间、次数与数据，否则新建条目。
    pub fn record(
        &mut self,
        capture: &Capture,
        analysis: &Analysis,
        thumbnail: Option<&Thumbnail>,
    ) -> Result<RecordOutcome> {
        let encoded = codec::encode(&capture.snapshot)?;
        let byte_size = capture.snapshot.total_bytes() as i64;
        let (inline, external) = self.write_payload(&encoded)?;

        let tx = self.conn.transaction()?;
        let existing: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT i.id, p.external_file FROM items i LEFT JOIN payloads p ON p.item_id = i.id
                 WHERE i.checksum = ?1",
                [&analysis.checksum],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let (id, inserted, stale_file) = match existing {
            Some((id, old_file)) => {
                tx.execute(
                    "UPDATE items SET last_used_at = ?2, copy_count = copy_count + 1,
                         source_bundle_id = ?3, source_name = ?4, byte_size = ?5
                     WHERE id = ?1",
                    params![
                        id,
                        capture.captured_at_ms,
                        capture.source.bundle_id,
                        capture.source.name,
                        byte_size
                    ],
                )?;
                (id, false, old_file)
            }
            None => {
                tx.execute(
                    "INSERT INTO items (checksum, kind, title, search_text, source_bundle_id, source_name,
                         created_at, last_used_at, byte_size, file_count, image_width, image_height, thumbnail)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        analysis.checksum,
                        analysis.kind.as_i64(),
                        analysis.title,
                        analysis.text,
                        capture.source.bundle_id,
                        capture.source.name,
                        capture.captured_at_ms,
                        byte_size,
                        analysis.file_paths.len() as i64,
                        thumbnail.map(|t| t.width),
                        thumbnail.map(|t| t.height),
                        thumbnail.map(|t| t.png.as_slice()),
                    ],
                )?;
                (tx.last_insert_rowid(), true, None)
            }
        };
        tx.execute(
            "INSERT INTO payloads (item_id, inline_data, external_file) VALUES (?1, ?2, ?3)
             ON CONFLICT(item_id) DO UPDATE SET inline_data = excluded.inline_data,
                 external_file = excluded.external_file",
            params![id, inline, external],
        )?;
        tx.commit()?;

        if let Some(file) = stale_file {
            self.remove_blob(&file);
        }
        Ok(RecordOutcome { id, inserted })
    }

    /// 小数据内联存进数据库，大数据写到外部文件：返回（内联数据，外部文件名）。
    fn write_payload(&self, encoded: &[u8]) -> Result<(Option<Vec<u8>>, Option<String>)> {
        if encoded.len() <= EXTERNAL_THRESHOLD {
            return Ok((Some(encoded.to_vec()), None));
        }
        let name = format!("{:x}-{:x}.zst", now_ms(), rand_suffix());
        let path = self.blob_dir.join(&name);
        fs::write(&path, encoded)
            .with_context(|| format!("写入外部数据失败：{}", path.display()))?;
        Ok((None, Some(name)))
    }

    /// 删除外部数据文件（文件已不存在时不报错）。
    fn remove_blob(&self, name: &str) {
        // 文件名只来自本库生成的值，这里仍防御性地拒绝路径分隔符。
        if name.contains('/') || name.contains("..") {
            return;
        }
        if let Err(err) = fs::remove_file(self.blob_dir.join(name)) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("删除外部数据 {name} 失败：{err}");
            }
        }
    }

    /// 按条件查询条目，最近使用的在前；`limit` 为 0 时最多返回 500 条。
    pub fn items(&self, query: &ItemQuery) -> Result<Vec<ClipItem>> {
        // 文件条目的 search_text 是按行拼接的路径，只取第一行，不把整段文本读出来。
        let mut sql = format!(
            "SELECT i.id, i.kind, i.title, i.source_bundle_id, i.source_name, i.created_at,
                    i.last_used_at, i.copy_count, i.paste_count, i.byte_size, i.file_count,
                    i.image_width, i.image_height, i.thumbnail IS NOT NULL,
                    (SELECT group_concat(list_id) FROM list_items WHERE item_id = i.id),
                    CASE WHEN i.kind = {files} THEN
                        substr(i.search_text, 1, instr(i.search_text || char(10), char(10)) - 1)
                    END
             FROM items i WHERE 1 = 1",
            files = ContentKind::Files.as_i64(),
        );
        let (condition, mut values) = filter_clause(query);
        sql.push_str(&condition);
        sql.push_str(" ORDER BY i.last_used_at DESC, i.id DESC LIMIT ?");
        let limit = if query.limit == 0 { 500 } else { query.limit };
        values.push(Value::Integer(limit as i64));

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(values), |row| {
            let width: Option<u32> = row.get(11)?;
            let height: Option<u32> = row.get(12)?;
            let lists: Option<String> = row.get(14)?;
            let first_path: Option<String> = row.get(15)?;
            Ok(ClipItem {
                id: row.get(0)?,
                kind: ContentKind::from_i64(row.get(1)?),
                title: row.get(2)?,
                source: SourceApp {
                    bundle_id: row.get(3)?,
                    name: row.get(4)?,
                },
                created_at_ms: row.get(5)?,
                last_used_at_ms: row.get(6)?,
                copy_count: row.get(7)?,
                paste_count: row.get(8)?,
                byte_size: row.get(9)?,
                file_count: row.get(10)?,
                first_path: first_path.filter(|path| !path.is_empty()),
                image_size: width.zip(height),
                has_thumbnail: row.get(13)?,
                list_ids: lists
                    .map(|s| s.split(',').filter_map(|id| id.parse().ok()).collect())
                    .unwrap_or_default(),
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// 满足条件的条目总数（不受 `limit` 限制，面板底部显示）。
    pub fn count(&self, query: &ItemQuery) -> Result<i64> {
        let (condition, values) = filter_clause(query);
        let sql = format!("SELECT count(*) FROM items i WHERE 1 = 1{condition}");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(stmt.query_row(params_from_iter(values), |row| row.get(0))?)
    }

    /// 条目的全文（预览、纯文本粘贴用）。
    pub fn text(&self, id: i64) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT search_text FROM items WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    /// 图片条目的缩略图（PNG）。
    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT thumbnail FROM items WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .flatten())
    }

    /// 读取完整快照（粘贴时写回系统剪贴板）。
    pub fn snapshot(&self, id: i64) -> Result<Snapshot> {
        let (inline, external): (Option<Vec<u8>>, Option<String>) = self
            .conn
            .query_row(
                "SELECT inline_data, external_file FROM payloads WHERE item_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .with_context(|| format!("找不到条目 {id} 的数据"))?;
        let encoded = match (inline, external) {
            (Some(data), _) => data,
            (None, Some(file)) => {
                let path = self.blob_dir.join(&file);
                fs::read(&path).with_context(|| format!("读取外部数据失败：{}", path.display()))?
            }
            (None, None) => anyhow::bail!("条目 {id} 没有数据"),
        };
        codec::decode(&encoded)
    }

    /// 粘贴后更新统计；`move_to_top` 为 true 时同时把条目移到最前。
    pub fn mark_pasted(&self, id: i64, move_to_top: bool) -> Result<()> {
        if move_to_top {
            self.conn.execute(
                "UPDATE items SET paste_count = paste_count + 1, last_used_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )?;
        } else {
            self.conn.execute(
                "UPDATE items SET paste_count = paste_count + 1 WHERE id = ?1",
                [id],
            )?;
        }
        Ok(())
    }

    /// 删除条目及其外部数据文件。
    pub fn delete(&mut self, id: i64) -> Result<()> {
        self.delete_where("i.id = ?", vec![Value::Integer(id)])?;
        Ok(())
    }

    /// 清空历史：保留已加入任意列表的条目。
    pub fn clear_history(&mut self) -> Result<usize> {
        self.delete_where(
            "NOT EXISTS (SELECT 1 FROM list_items li WHERE li.item_id = i.id)",
            Vec::new(),
        )
    }

    /// 按保留时长与最大条数清理历史；列表中的条目不受影响。
    pub fn prune(
        &mut self,
        now_ms: i64,
        retention_ms: Option<i64>,
        max_items: Option<usize>,
    ) -> Result<usize> {
        let mut removed = 0;
        /// 不在任何列表中的条目。
        const NOT_IN_LIST: &str =
            "NOT EXISTS (SELECT 1 FROM list_items li WHERE li.item_id = i.id)";
        if let Some(retention) = retention_ms {
            removed += self.delete_where(
                &format!("{NOT_IN_LIST} AND i.last_used_at < ?"),
                vec![Value::Integer(now_ms - retention)],
            )?;
        }
        if let Some(max) = max_items {
            removed += self.delete_where(
                &format!(
                    "{NOT_IN_LIST} AND i.id NOT IN (SELECT i.id FROM items i WHERE {NOT_IN_LIST}
                       ORDER BY i.last_used_at DESC, i.id DESC LIMIT ?)"
                ),
                vec![Value::Integer(max as i64)],
            )?;
        }
        Ok(removed)
    }

    /// 删除满足条件的条目（`condition` 中条目表的别名是 `i`），并清理它们的外部数据文件。返回删除的条数。
    fn delete_where(&mut self, condition: &str, values: Vec<Value>) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let files: Vec<String> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT p.external_file FROM items i JOIN payloads p ON p.item_id = i.id
                 WHERE p.external_file IS NOT NULL AND {condition}"
            ))?;
            let rows = stmt.query_map(params_from_iter(values.iter()), |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let removed = tx.execute(
            &format!("DELETE FROM items WHERE id IN (SELECT i.id FROM items i WHERE {condition})"),
            params_from_iter(values.iter()),
        )?;
        tx.commit()?;
        for file in files {
            self.remove_blob(&file);
        }
        Ok(removed)
    }

    /// 所有列表及其条目数，按创建顺序（“收藏”在最前）。
    pub fn lists(&self) -> Result<Vec<ClipList>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT l.id, l.name, (SELECT count(*) FROM list_items li WHERE li.list_id = l.id)
             FROM lists l ORDER BY l.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ClipList {
                id: row.get(0)?,
                name: row.get(1)?,
                item_count: row.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// 新建列表，返回列表 id；名字为空或已存在时报错。
    pub fn create_list(&self, name: &str) -> Result<i64> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "列表名称不能为空");
        self.conn
            .execute(
                "INSERT INTO lists (name, created_at) VALUES (?1, ?2)",
                params![name, now_ms()],
            )
            .with_context(|| format!("列表“{name}”已存在"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 重命名列表；名字为空或与其他列表重名时报错。
    pub fn rename_list(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "列表名称不能为空");
        self.conn
            .execute(
                "UPDATE lists SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .with_context(|| format!("列表“{name}”已存在"))?;
        Ok(())
    }

    /// 删除列表本身，条目仍保留在历史中。
    pub fn delete_list(&self, id: i64) -> Result<()> {
        anyhow::ensure!(id != FAVORITES_LIST_ID, "内置的“收藏”列表不能删除");
        self.conn.execute("DELETE FROM lists WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 把条目加入（`member` 为 true）或移出列表。
    pub fn set_in_list(&self, item_id: i64, list_id: i64, member: bool) -> Result<()> {
        if member {
            self.conn.execute(
                "INSERT OR IGNORE INTO list_items (list_id, item_id, added_at) VALUES (?1, ?2, ?3)",
                params![list_id, item_id, now_ms()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM list_items WHERE list_id = ?1 AND item_id = ?2",
                params![list_id, item_id],
            )?;
        }
        Ok(())
    }

    /// 最近出现过的来源应用（设置页“排除应用”的候选）。
    pub fn recent_sources(&self, limit: usize) -> Result<Vec<SourceApp>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT source_bundle_id, max(source_name) FROM items
             WHERE source_bundle_id IS NOT NULL
             GROUP BY source_bundle_id ORDER BY max(last_used_at) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(SourceApp {
                bundle_id: row.get(0)?,
                name: row.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// 条目数与占用的磁盘空间（数据库文件、WAL 文件与外部数据）。
    pub fn stats(&self) -> Result<StoreStats> {
        let item_count = self
            .conn
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))?;
        let database_bytes = [DB_FILE, "history.sqlite3-wal"]
            .iter()
            .filter_map(|name| fs::metadata(self.dir.join(name)).ok())
            .map(|meta| meta.len())
            .sum();
        let external_bytes = fs::read_dir(&self.blob_dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok()?.metadata().ok())
                    .map(|meta| meta.len())
                    .sum()
            })
            .unwrap_or(0);
        Ok(StoreStats {
            item_count,
            database_bytes,
            external_bytes,
        })
    }
}

/// 切换到 WAL 模式。新建的数据库切换时需要独占锁，且 SQLite 不会为此调用 busy 等待，
/// 多个连接同时打开时会直接返回 busy，因此这里自己重试。
fn enable_wal(conn: &Connection) -> Result<()> {
    let mut attempts = 0;
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::DatabaseBusy && attempts < 100 =>
            {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(err) => return Err(err).context("切换 WAL 模式失败"),
        }
    }
}

/// 查询条件对应的 SQL 片段（以 ` AND` 开头）与参数。
fn filter_clause(query: &ItemQuery) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut values = Vec::new();
    match query.kind {
        Some(KindFilter::Textual) => {
            sql.push_str(" AND i.kind IN (?, ?)");
            values.push(Value::Integer(ContentKind::Text.as_i64()));
            values.push(Value::Integer(ContentKind::RichText.as_i64()));
        }
        Some(KindFilter::Exact(kind)) => {
            sql.push_str(" AND i.kind = ?");
            values.push(Value::Integer(kind.as_i64()));
        }
        None => {}
    }
    if let Some(list_id) = query.list_id {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM list_items li WHERE li.list_id = ? AND li.item_id = i.id)",
        );
        values.push(Value::Integer(list_id));
    }
    for term in query.text.split_whitespace() {
        sql.push_str(
            " AND (i.search_text LIKE ? ESCAPE '\\' OR i.title LIKE ? ESCAPE '\\'
                   OR i.source_name LIKE ? ESCAPE '\\')",
        );
        let pattern = format!("%{}%", escape_like(term));
        values.extend(std::iter::repeat_n(Value::Text(pattern), 3));
    }
    (sql, values)
}

/// 转义 LIKE 中的通配符 `%`、`_` 与转义符本身，让关键字按字面匹配。
fn escape_like(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for ch in term.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 外部数据文件名的随机后缀（同一毫秒内写入多个文件也不会重名）。
fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Representation;
    use crate::uti::{analyze, PNG};

    fn capture(snapshot: Snapshot, at: i64) -> Capture {
        Capture {
            snapshot,
            source: SourceApp {
                bundle_id: Some("com.example.app".into()),
                name: Some("Example".into()),
            },
            captured_at_ms: at,
        }
    }

    fn record_text(store: &mut Store, text: &str, at: i64) -> RecordOutcome {
        let capture = capture(Snapshot::from_text(text), at);
        let analysis = analyze(&capture.snapshot);
        store.record(&capture, &analysis, None).unwrap()
    }

    fn all(store: &Store) -> Vec<ClipItem> {
        store.items(&ItemQuery::default()).unwrap()
    }

    #[test]
    fn records_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let a = record_text(&mut store, "alpha", 1_000);
        let b = record_text(&mut store, "beta", 2_000);
        let again = record_text(&mut store, "alpha", 3_000);
        assert!(a.inserted && b.inserted && !again.inserted);
        assert_eq!(a.id, again.id);

        let items = all(&store);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "alpha", "重复复制后应排到最前");
        assert_eq!(items[0].copy_count, 2);
        assert_eq!(items[0].created_at_ms, 1_000);
        assert_eq!(items[0].last_used_at_ms, 3_000);
        assert_eq!(store.snapshot(a.id).unwrap(), Snapshot::from_text("alpha"));
    }

    #[test]
    fn searches_with_multiple_terms_and_escapes_like() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        record_text(&mut store, "SELECT * FROM users", 1);
        record_text(&mut store, "select name from orders", 2);
        record_text(&mut store, "100% done_ok", 3);
        record_text(&mut store, "中文 剪贴板 内容", 4);

        let search = |text: &str| {
            store
                .items(&ItemQuery {
                    text: text.into(),
                    ..Default::default()
                })
                .unwrap()
                .into_iter()
                .map(|item| item.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(search("select users"), vec!["SELECT * FROM users"]);
        assert_eq!(search("from").len(), 2);
        assert_eq!(search("0%"), vec!["100% done_ok"]);
        assert_eq!(search("e_o"), vec!["100% done_ok"]);
        assert!(search("e%o").is_empty());
        assert_eq!(search("剪贴"), vec!["中文 剪贴板 内容"]);
        assert_eq!(search("example").len(), 4, "来源应用名也参与搜索");
        let count = |text: &str| {
            store
                .count(&ItemQuery {
                    text: text.into(),
                    limit: 1,
                    ..Default::default()
                })
                .unwrap()
        };
        assert_eq!(count("from"), 2, "计数不受 limit 影响");
        assert_eq!(count(""), 4);
    }

    #[test]
    fn filters_by_kind_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let text = record_text(&mut store, "plain", 1);
        record_text(&mut store, "https://example.com", 2);
        store.set_in_list(text.id, FAVORITES_LIST_ID, true).unwrap();

        let query = |kind, list_id| {
            store
                .items(&ItemQuery {
                    kind,
                    list_id,
                    ..Default::default()
                })
                .unwrap()
        };
        assert_eq!(query(Some(KindFilter::Textual), None).len(), 1);
        assert_eq!(
            query(Some(KindFilter::Exact(ContentKind::Link)), None).len(),
            1
        );
        let favorites = query(None, Some(FAVORITES_LIST_ID));
        assert_eq!(favorites.len(), 1);
        assert!(favorites[0].in_list(FAVORITES_LIST_ID));
        assert_eq!(store.lists().unwrap()[0].item_count, 1);
    }

    #[test]
    fn lists_first_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let files = Snapshot {
            items: vec![
                vec![Representation::new(
                    crate::uti::FILE_URL,
                    b"file:///Volumes/E/download/%E5%B7%A5%E4%BD%9C%E7%B0%BF1.xlsx".to_vec(),
                )],
                vec![Representation::new(
                    crate::uti::FILE_URL,
                    b"file:///tmp/b.txt".to_vec(),
                )],
            ],
        };
        let capture = capture(files, 1);
        store
            .record(&capture, &analyze(&capture.snapshot), None)
            .unwrap();
        record_text(&mut store, "plain", 2);

        let items = all(&store);
        assert_eq!(items[0].first_path, None, "文本条目没有路径");
        assert_eq!(items[1].file_count, 2);
        assert_eq!(
            items[1].first_path.as_deref(),
            Some("/Volumes/E/download/工作簿1.xlsx")
        );
    }

    #[test]
    fn large_payload_goes_external_and_is_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        // 伪随机数据不可压缩，确保超过外置阈值。
        let mut state = 0x2545F4914F6CDD1Du64;
        let noise: Vec<u8> = (0..EXTERNAL_THRESHOLD * 2)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let snapshot = Snapshot {
            items: vec![vec![Representation::new(PNG, noise)]],
        };
        let capture = capture(snapshot.clone(), 1);
        let analysis = analyze(&snapshot);
        let outcome = store.record(&capture, &analysis, None).unwrap();

        let blobs = || fs::read_dir(dir.path().join(BLOB_DIR)).unwrap().count();
        assert_eq!(blobs(), 1);
        assert_eq!(store.snapshot(outcome.id).unwrap(), snapshot);
        assert!(store.stats().unwrap().external_bytes > 0);

        // 再次复制同一内容：旧的外部文件被替换，不会泄漏。
        store.record(&capture, &analysis, None).unwrap();
        assert_eq!(blobs(), 1);

        store.delete(outcome.id).unwrap();
        assert_eq!(blobs(), 0);
        assert!(all(&store).is_empty());
    }

    #[test]
    fn prune_respects_lists_retention_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let old_fav = record_text(&mut store, "old favorite", 10);
        record_text(&mut store, "old", 20);
        for i in 0..5 {
            record_text(&mut store, &format!("new {i}"), 1_000 + i);
        }
        store
            .set_in_list(old_fav.id, FAVORITES_LIST_ID, true)
            .unwrap();

        let removed = store.prune(1_100, Some(500), None).unwrap();
        assert_eq!(removed, 1, "只删除过期且不在列表中的条目");
        let removed = store.prune(1_100, None, Some(2)).unwrap();
        assert_eq!(removed, 3);
        let titles: Vec<_> = all(&store).into_iter().map(|item| item.title).collect();
        assert_eq!(titles, vec!["new 4", "new 3", "old favorite"]);

        assert_eq!(store.clear_history().unwrap(), 2);
        assert_eq!(all(&store).len(), 1);
    }

    #[test]
    fn mark_pasted_moves_to_top_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let first = record_text(&mut store, "first", 1);
        record_text(&mut store, "second", 2);
        store.mark_pasted(first.id, false).unwrap();
        assert_eq!(all(&store)[0].title, "second");
        store.mark_pasted(first.id, true).unwrap();
        let items = all(&store);
        assert_eq!(items[0].title, "first");
        assert_eq!(items[0].paste_count, 2);
    }

    #[test]
    fn list_management() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let id = store.create_list("SQL 片段").unwrap();
        assert!(store.create_list("SQL 片段").is_err());
        assert!(store.create_list("  ").is_err());
        store.rename_list(id, "SQL").unwrap();
        assert_eq!(
            store
                .lists()
                .unwrap()
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            ["收藏", "SQL"]
        );
        assert!(store.delete_list(FAVORITES_LIST_ID).is_err());
        store.delete_list(id).unwrap();
        assert_eq!(store.lists().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_open_on_fresh_directory() {
        let dir = tempfile::tempdir().unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = dir.path().to_path_buf();
                std::thread::spawn(move || Store::open(&path).map(|_| ()))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert_eq!(Store::open(dir.path()).unwrap().lists().unwrap().len(), 1);
    }

    #[test]
    fn reopen_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut store = Store::open(dir.path()).unwrap();
            record_text(&mut store, "persisted", 1);
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(all(&store)[0].title, "persisted");
        assert_eq!(
            store.recent_sources(5).unwrap()[0].bundle_id.as_deref(),
            Some("com.example.app")
        );
    }
}
