//! 每根快照：格式、校验与 manifest（§7.6）。
//!
//! - 快照不替代 live 索引，不保存 UI 状态；
//! - 文件布局：`<root_key>.<generation>.bin` + `manifest.json`（格式版本、当前代数）；
//! - 头部：magic(4) + 格式版本 u32(LE) + SHA-256(32) + zstd 压缩的 postcard 载荷；
//! - 保存：临时文件 → fsync → 原子替换；加载后重建派生表、校验身份与配置指纹。

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::index::{Columns, NameArena, RootIndex};
use crate::model::{IndexConfig, RootLocation};

/// 快照 magic。
pub const SNAPSHOT_MAGIC: [u8; 4] = *b"DSIX";
/// 快照格式版本。
pub const FORMAT_VERSION: u32 = 1;
/// 头部长度：magic(4) + version(4) + sha256(32)。
const HEADER_LEN: usize = 40;
/// 单根索引的解压预算，超限时由运行时重建索引。
const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024 * 1024;

/// 事件流检查点（§7.6：重连后用 `last_event_id` 作 catch-up 起点）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCheckpoint {
    pub last_event_id: u64,
    pub stream_id: String,
}

/// 快照载荷（postcard 序列化）。
#[derive(Serialize, Deserialize)]
struct SnapshotBody<I = RootLocation, C = Columns, N = NameArena, E = EventCheckpoint> {
    identity: I,
    config_fingerprint: u64,
    epoch: u32,
    revision: u64,
    checkpoint: E,
    next_node: u32,
    columns: C,
    names: N,
}

/// 快照解码结果。
pub struct DecodedSnapshot {
    pub index: RootIndex,
    pub identity: RootLocation,
    pub config_fingerprint: u64,
    pub checkpoint: EventCheckpoint,
    /// 通过存储加载时的快照文件代数。内存解码时没有 manifest。
    pub storage_generation: Option<u64>,
}

/// 快照错误。
#[derive(Debug)]
pub enum SnapshotError {
    Io(io::Error),
    /// magic / 版本 / 校验和不匹配（损坏或格式升级）。
    Invalid(&'static str),
    Decode(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Io(e) => write!(f, "快照 IO 错误：{e}"),
            SnapshotError::Invalid(why) => write!(f, "快照无效：{why}"),
            SnapshotError::Decode(e) => write!(f, "快照解码失败：{e}"),
        }
    }
}

impl From<io::Error> for SnapshotError {
    fn from(e: io::Error) -> Self {
        SnapshotError::Io(e)
    }
}

/// 编码一个根索引为快照字节。
pub fn encode(
    index: &RootIndex,
    identity: &RootLocation,
    config_fingerprint: u64,
    checkpoint: &EventCheckpoint,
) -> Result<Vec<u8>, SnapshotError> {
    let mut out = Cursor::new(Vec::new());
    write_snapshot(&mut out, index, identity, config_fingerprint, checkpoint)?;
    Ok(out.into_inner())
}

/// 压缩产物直接进目标文件，哈希与写入同步计算。留出头部，压缩结束后回填校验和。
fn write_snapshot(
    mut out: impl Write + Seek,
    index: &RootIndex,
    identity: &RootLocation,
    config_fingerprint: u64,
    checkpoint: &EventCheckpoint,
) -> Result<(), SnapshotError> {
    let (columns, names, next_node) = index.parts();
    let body = SnapshotBody {
        identity,
        config_fingerprint,
        epoch: index.epoch(),
        revision: index.revision(),
        checkpoint,
        next_node,
        columns,
        names,
    };
    out.write_all(&SNAPSHOT_MAGIC)?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())?;
    out.write_all(&[0; 32])?;
    let mut compressed = HashWriter {
        out: &mut out,
        hash: Sha256::new(),
    };
    let encoder = zstd::stream::write::Encoder::new(&mut compressed, 3)?;
    let mut writer = BufWriter::with_capacity(64 * 1024, encoder);
    postcard::to_io(&body, &mut writer).map_err(|e| SnapshotError::Decode(e.to_string()))?;
    writer.into_inner().map_err(|e| e.into_error())?.finish()?;
    let checksum = compressed.hash.finalize();
    out.seek(SeekFrom::Start(8))?;
    out.write_all(&checksum)?;
    Ok(())
}

struct HashWriter<W> {
    out: W,
    hash: Sha256,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.out.write(bytes)?;
        self.hash.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// 解码并校验快照；成功后派生表已重建。
pub fn decode(bytes: &[u8]) -> Result<DecodedSnapshot, SnapshotError> {
    if bytes.len() < HEADER_LEN {
        return Err(SnapshotError::Invalid("文件太短"));
    }
    if bytes[..4] != SNAPSHOT_MAGIC {
        return Err(SnapshotError::Invalid("magic 不匹配"));
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(SnapshotError::Invalid("格式版本不支持"));
    }
    let checksum = &bytes[8..40];
    let compressed = &bytes[HEADER_LEN..];
    if Sha256::digest(compressed).as_slice() != checksum {
        return Err(SnapshotError::Invalid("校验和不匹配"));
    }
    let mut payload = Vec::new();
    let mut decoder = zstd::stream::read::Decoder::new(compressed)?;
    decoder.window_log_max(27)?;
    decoder
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut payload)?;
    if payload.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::Invalid("快照解压超过预算"));
    }
    let body: SnapshotBody =
        postcard::from_bytes(&payload).map_err(|e| SnapshotError::Decode(e.to_string()))?;
    drop(payload);
    RootIndex::validate_parts(&body.columns, &body.names, body.next_node)
        .map_err(SnapshotError::Invalid)?;
    let mut index = RootIndex::from_parts(
        body.columns,
        body.names,
        body.next_node,
        body.epoch,
        body.revision,
    );
    index.set_identity(body.identity.clone());
    Ok(DecodedSnapshot {
        index,
        identity: body.identity,
        config_fingerprint: body.config_fingerprint,
        checkpoint: body.checkpoint,
        storage_generation: None,
    })
}

/// 索引配置指纹（排除规则、大小写、app 包、监听等；§7.6 配置变化时快照失效）。
pub fn config_fingerprint(config: &IndexConfig) -> u64 {
    let relevant = (
        &config.exclude_rules,
        config.exclude_case_sensitive,
        &config.ignore_paths,
        config.scan_app_bundles,
        config.watch,
    );
    let bytes = postcard::to_allocvec(&relevant).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    u64::from_le_bytes(digest[..8].try_into().unwrap())
}

/// manifest 中一根的快照记录。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub generation: u64,
    pub epoch: u32,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<EventCheckpoint>,
}

/// manifest 文件结构。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default = "manifest_version")]
    version: u32,
    #[serde(default)]
    roots: BTreeMap<String, ManifestEntry>,
}

fn manifest_version() -> u32 {
    1
}

/// 快照存储（数据目录下的 `search/snapshots/`）。
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 保存一根的快照：临时文件 → fsync → 原子替换 → 更新 manifest → 清理旧代数。
    /// 返回新代数。
    pub fn save(
        &self,
        root_key: &str,
        index: &RootIndex,
        identity: &RootLocation,
        fingerprint: u64,
        checkpoint: &EventCheckpoint,
    ) -> Result<u64, SnapshotError> {
        self.save_if(root_key, index, identity, fingerprint, checkpoint, || true)
            .map(|generation| generation.expect("无条件保存"))
    }

    /// 先在锁外编码，再在发布前检查扫描是否已取消或缓存是否已清理。
    pub fn save_if(
        &self,
        root_key: &str,
        index: &RootIndex,
        identity: &RootLocation,
        fingerprint: u64,
        checkpoint: &EventCheckpoint,
        current: impl FnOnce() -> bool,
    ) -> Result<Option<u64>, SnapshotError> {
        fs::create_dir_all(&self.dir)?;
        let mut pending = tempfile::NamedTempFile::new_in(&self.dir)?;
        write_snapshot(
            pending.as_file_mut(),
            index,
            identity,
            fingerprint,
            checkpoint,
        )?;
        pending.as_file().sync_all()?;
        let _lock = self.lock_manifest()?;
        if !current() {
            return Ok(None);
        }

        let mut manifest = self.read_manifest()?;
        let previous = manifest.roots.get(root_key);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let generation = match previous {
            Some(entry) => now.max(
                entry
                    .generation
                    .checked_add(1)
                    .ok_or(SnapshotError::Invalid("快照代数耗尽"))?,
            ),
            None => now,
        };
        let path = self.snapshot_path(root_key, generation);
        pending.persist(&path).map_err(|err| err.error)?;

        let previous = manifest.roots.insert(
            root_key.to_string(),
            ManifestEntry {
                generation,
                epoch: index.epoch(),
                revision: index.revision(),
                checkpoint: None,
            },
        );
        if let Err(err) = self.write_manifest(&manifest) {
            let _ = fs::remove_file(&path);
            return Err(err);
        }
        if let Some(previous) = previous {
            let _ = fs::remove_file(self.snapshot_path(root_key, previous.generation));
        }
        Ok(Some(generation))
    }

    /// 读取一根的最新快照。只在解析指针和打开文件时持锁。
    pub fn load_latest(&self, root_key: &str) -> Result<Option<DecodedSnapshot>, SnapshotError> {
        let lock = match self.lock_manifest() {
            Ok(lock) => lock,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let manifest = self.read_manifest()?;
        let Some(entry) = manifest.roots.get(root_key) else {
            return Ok(None);
        };
        let file = match fs::File::open(self.snapshot_path(root_key, entry.generation)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SnapshotError::Io(e)),
        };
        drop(lock);
        let mut bytes = Vec::new();
        file.take(MAX_SNAPSHOT_BYTES + HEADER_LEN as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES + HEADER_LEN as u64 {
            return Err(SnapshotError::Invalid("快照文件超过读取预算"));
        }
        let mut snapshot = decode(&bytes)?;
        snapshot.storage_generation = Some(entry.generation);
        if let Some(checkpoint) = &entry.checkpoint {
            snapshot.checkpoint = checkpoint.clone();
        }
        Ok(Some(snapshot))
    }

    /// 回放未改变索引时只推进该快照的检查点。其他任务已经替换快照时放弃提交。
    pub fn advance_checkpoint(
        &self,
        root_key: &str,
        generation: u64,
        checkpoint: &EventCheckpoint,
        current: impl FnOnce() -> bool,
    ) -> Result<(), SnapshotError> {
        let _lock = self.lock_manifest()?;
        if !current() {
            return Ok(());
        }
        let mut manifest = self.read_manifest()?;
        let Some(entry) = manifest.roots.get_mut(root_key) else {
            return Ok(());
        };
        if entry.generation == generation
            && entry.checkpoint.as_ref().is_none_or(|previous| previous.last_event_id < checkpoint.last_event_id)
        {
            entry.checkpoint = Some(checkpoint.clone());
            self.write_manifest(&manifest)?;
        }
        Ok(())
    }

    pub fn remove(&self, root_key: &str) -> Result<(), SnapshotError> {
        self.remove_matching(|key| key == root_key)
    }

    /// 同一事务清理一组根，也包含先前损坏的 manifest 遗留的文件。
    pub fn remove_roots(&self, keys: &[String]) -> Result<(), SnapshotError> {
        self.remove_matching(|key| keys.iter().any(|known| known == key))
    }

    pub fn clean_orphans(&self, known: &[String]) -> Result<(), SnapshotError> {
        self.remove_matching(|key| !known.iter().any(|known| known == key))
    }

    fn remove_matching(&self, remove: impl Fn(&str) -> bool) -> Result<(), SnapshotError> {
        let _lock = match self.lock_manifest() {
            Ok(lock) => lock,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut manifest = self.read_manifest()?;
        let old_len = manifest.roots.len();
        manifest.roots.retain(|key, _| !remove(key));
        if manifest.roots.len() != old_len {
            self.write_manifest(&manifest)?;
        }
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let key = name.to_str().and_then(|name| {
                let (key, generation) = name.strip_suffix(".bin")?.rsplit_once('.')?;
                generation.parse::<u64>().ok().map(|_| key)
            });
            if key.is_some_and(&remove) {
                match fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }

    fn snapshot_path(&self, root_key: &str, generation: u64) -> PathBuf {
        self.dir.join(format!("{root_key}.{generation}.bin"))
    }

    fn lock_manifest(&self) -> io::Result<fs::File> {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join("manifest.lock"))?;
        lock.lock()?;
        Ok(lock)
    }

    fn read_manifest(&self) -> Result<Manifest, SnapshotError> {
        match fs::read(self.dir.join("manifest.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn write_manifest(&self, manifest: &Manifest) -> Result<(), SnapshotError> {
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|e| SnapshotError::Decode(e.to_string()))?;
        atomic_write(&self.dir.join("manifest.json"), &bytes)?;
        Ok(())
    }
}

/// 原子写：唯一临时文件 → fsync → 原子替换。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    pending.write_all(bytes)?;
    pending.as_file().sync_all()?;
    pending.persist(path).map_err(|err| err.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, NodeMeta, RootId, VolumeId};
    use std::ffi::OsStr;

    #[test]
    fn concurrent_root_writes_keep_both_manifest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for key in ["first", "second"] {
                let path = dir.path().to_owned();
                let barrier = &barrier;
                scope.spawn(move || {
                    let store = SnapshotStore::new(path);
                    let mut index = RootIndex::new(RootId(0));
                    index.insert(None, OsStr::new(key), NodeKind::Dir, NodeMeta::default());
                    index.commit();
                    barrier.wait();
                    store
                        .save(key, &index, &identity(), 7, &EventCheckpoint::default())
                        .unwrap();
                });
            }
        });
        let store = SnapshotStore::new(dir.path().to_owned());
        for key in ["first", "second"] {
            let snapshot = store.load_latest(key).unwrap().unwrap();
            assert_eq!(snapshot.index.raw_name_str(crate::NodeId::ROOT), Some(key));
        }
    }

    #[test]
    fn cancelled_publish_does_not_replace_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_owned());
        let mut index = RootIndex::new(RootId(0));
        index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
        index.commit();
        store
            .save("root", &index, &identity(), 7, &EventCheckpoint::default())
            .unwrap();
        let result = store.save_if(
            "root",
            &index,
            &identity(),
            8,
            &EventCheckpoint::default(),
            || false,
        );
        assert_eq!(result.unwrap(), None);
        assert_eq!(
            store
                .load_latest("root")
                .unwrap()
                .unwrap()
                .config_fingerprint,
            7
        );
    }

    #[test]
    fn an_unchanged_snapshot_advances_its_checkpoint_without_rewriting_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().to_owned());
        let mut index = RootIndex::new(RootId(0));
        index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
        index.commit();
        let generation = store.save("root", &index, &identity(), 7, &EventCheckpoint::default()).unwrap();
        let path = store.snapshot_path("root", generation);
        let original = fs::read(&path).unwrap();
        let checkpoint = EventCheckpoint { last_event_id: 10, stream_id: "stream".into() };
        store.advance_checkpoint("root", generation, &checkpoint, || true).unwrap();
        let loaded = store.load_latest("root").unwrap().unwrap();
        assert_eq!(loaded.storage_generation, Some(generation));
        assert_eq!(loaded.checkpoint, checkpoint);
        assert_eq!(fs::read(&path).unwrap(), original);

        store.save("root", &index, &identity(), 7, &EventCheckpoint::default()).unwrap();
        store.advance_checkpoint("root", generation, &checkpoint, || true).unwrap();
        assert_eq!(store.load_latest("root").unwrap().unwrap().checkpoint, EventCheckpoint::default());
    }

    fn identity() -> RootLocation {
        RootLocation {
            volume_id: VolumeId("vol-1".into()),
            relative_path: "Users/me".into(),
            fallback_absolute_path: PathBuf::from("/Users/me"),
        }
    }

    #[test]
    fn snapshot_round_trip_preserves_index() {
        let mut index = RootIndex::new(RootId(0));
        let root = index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
        index.insert(
            Some(root),
            OsStr::new("报告.txt"),
            NodeKind::File,
            NodeMeta {
                size: Some(42),
                mtime: Some(1_700_000_000),
                btime: None,
            },
        );
        index.commit();
        let id = identity();
        let bytes = encode(&index, &id, 7, &EventCheckpoint { last_event_id: 99, stream_id: "s".into() }).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.index.len(), 2);
        assert_eq!(decoded.identity, id);
        assert_eq!(decoded.config_fingerprint, 7);
        assert_eq!(decoded.checkpoint.last_event_id, 99);
        // 派生表已重建：rank 与默认路径序可用。
        assert!(decoded.index.derived_valid());
        assert_eq!(decoded.index.revision(), index.revision());
        assert_eq!(decoded.index.epoch(), index.epoch());
    }

    #[test]
    fn rejects_corrupted_and_unknown_versions() {
        let mut index = RootIndex::new(RootId(0));
        index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
        index.commit();
        let bytes = encode(&index, &identity(), 0, &EventCheckpoint::default()).unwrap();
        assert!(matches!(decode(&bytes[..20]), Err(SnapshotError::Invalid(_))));
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(matches!(decode(&bad), Err(SnapshotError::Invalid(_))));
        let mut bad = bytes.clone();
        bad[4] = 99;
        assert!(matches!(decode(&bad), Err(SnapshotError::Invalid(_))));
        let mut bad = bytes;
        *bad.last_mut().unwrap() ^= 0xff;
        assert!(matches!(decode(&bad), Err(SnapshotError::Invalid("校验和不匹配"))));
    }

    #[test]
    fn store_save_load_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path().join("snapshots"));
        let mut index = RootIndex::new(RootId(0));
        index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
        index.commit();
        let id = identity();
        let key = crate::volumes::root_key(&id);
        assert!(store.load_latest(&key).unwrap().is_none());
        store.save(&key, &index, &id, 0, &EventCheckpoint::default()).unwrap();
        let loaded = store.load_latest(&key).unwrap().unwrap();
        assert_eq!(loaded.index.len(), 1);
        // 保存新代数后旧文件被清理。
        store.save(&key, &index, &id, 0, &EventCheckpoint::default()).unwrap();
        let bins = fs::read_dir(store.dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bin"))
            .count();
        assert_eq!(bins, 1);
        store.remove(&key).unwrap();
        assert!(store.load_latest(&key).unwrap().is_none());
    }
}
