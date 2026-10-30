//! 每根快照：格式、校验与 manifest（§7.6）。
//!
//! - 快照不替代 live 索引，不保存 UI 状态；
//! - 文件布局：`<root_key>.<generation>.bin` + `manifest.json`（格式版本、当前代数）；
//! - 头部：magic(4) + 格式版本 u32(LE) + SHA-256(32) + zstd 压缩的 postcard 载荷；
//! - 保存：临时文件 → fsync → 原子替换；加载后重建派生表、校验身份与配置指纹。

use std::collections::BTreeMap;
use std::fs;
use std::io;
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

/// 事件流检查点（§7.6：重连后用 `last_event_id` 作 catch-up 起点）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCheckpoint {
    pub last_event_id: u64,
    pub stream_id: String,
}

/// 快照载荷（postcard 序列化）。
#[derive(Serialize, Deserialize)]
struct SnapshotBody {
    identity: RootLocation,
    config_fingerprint: u64,
    epoch: u32,
    revision: u64,
    checkpoint: EventCheckpoint,
    next_node: u32,
    columns: Columns,
    names: NameArena,
}

/// 快照解码结果。
pub struct DecodedSnapshot {
    pub index: RootIndex,
    pub identity: RootLocation,
    pub config_fingerprint: u64,
    pub checkpoint: EventCheckpoint,
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
    let (columns, names, next_node) = index.parts();
    let body = SnapshotBody {
        identity: identity.clone(),
        config_fingerprint,
        epoch: index.epoch(),
        revision: index.revision(),
        checkpoint: checkpoint.clone(),
        next_node,
        columns: columns.clone(),
        // NameArena 含 `#[serde(skip)]` 的去重索引，不随快照保存。
        names: names.clone(),
    };
    let payload = postcard::to_allocvec(&body).map_err(|e| SnapshotError::Decode(e.to_string()))?;
    let compressed = zstd::encode_all(&payload[..], 3)?;
    let checksum = Sha256::digest(&compressed);
    let mut out = Vec::with_capacity(HEADER_LEN + compressed.len());
    out.extend_from_slice(&SNAPSHOT_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&compressed);
    Ok(out)
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
    let payload = zstd::decode_all(compressed)?;
    let body: SnapshotBody =
        postcard::from_bytes(&payload).map_err(|e| SnapshotError::Decode(e.to_string()))?;
    let identity = body.identity.clone();
    let index = RootIndex::from_parts(
        body.columns,
        body.names,
        body.next_node,
        body.epoch,
        body.revision,
    );
    Ok(DecodedSnapshot {
        index,
        identity,
        config_fingerprint: body.config_fingerprint,
        checkpoint: body.checkpoint,
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
        fs::create_dir_all(&self.dir)?;
        let bytes = encode(index, identity, fingerprint, checkpoint)?;
        let generation = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let file = self.dir.join(format!("{root_key}.{generation}.bin"));
        atomic_write(&file, &bytes)?;
        // 更新 manifest。
        let mut manifest = self.read_manifest();
        manifest.roots.insert(
            root_key.to_string(),
            ManifestEntry {
                generation,
                epoch: index.epoch(),
                revision: index.revision(),
            },
        );
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|e| SnapshotError::Decode(e.to_string()))?;
        atomic_write(&self.dir.join("manifest.json"), &manifest_bytes)?;
        // 清理同根的旧代数。
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(&format!("{root_key}."))
                    && name.ends_with(".bin")
                    && entry.path() != file
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(generation)
    }

    /// 读取一根的最新快照（manifest 指向的代数）。不存在返回 `Ok(None)`。
    pub fn load_latest(&self, root_key: &str) -> Result<Option<DecodedSnapshot>, SnapshotError> {
        let manifest = self.read_manifest();
        let Some(entry) = manifest.roots.get(root_key) else {
            return Ok(None);
        };
        let file = self.dir.join(format!("{root_key}.{}.bin", entry.generation));
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SnapshotError::Io(e)),
        };
        decode(&bytes).map(Some)
    }

    /// 移除一根的快照与 manifest 记录。
    pub fn remove(&self, root_key: &str) -> Result<(), SnapshotError> {
        let mut manifest = self.read_manifest();
        if let Some(entry) = manifest.roots.remove(root_key) {
            let file = self.dir.join(format!("{root_key}.{}.bin", entry.generation));
            let _ = fs::remove_file(file);
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)
                .map_err(|e| SnapshotError::Decode(e.to_string()))?;
            atomic_write(&self.dir.join("manifest.json"), &manifest_bytes)?;
        }
        Ok(())
    }

    /// 清理不再存在的根的快照文件。
    pub fn clean_orphans(&self, known: &[String]) -> Result<(), SnapshotError> {
        let manifest = self.read_manifest();
        let orphans: Vec<String> = manifest
            .roots
            .keys()
            .filter(|k| !known.contains(k))
            .cloned()
            .collect();
        for key in orphans {
            self.remove(&key)?;
        }
        Ok(())
    }

    fn read_manifest(&self) -> Manifest {
        fs::read(self.dir.join("manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

/// 原子写：临时文件 → fsync → 原子替换（§7.6）。
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "snapshot".into());
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    {
        let mut file = fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, NodeMeta, RootId, VolumeId};
    use std::ffi::OsStr;

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
