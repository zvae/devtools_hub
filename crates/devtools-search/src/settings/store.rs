//! 搜索设置存储：原子 JSON 保存、损坏隔离、版本检查（§6.2）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::model::{SearchSettings, SETTINGS_VERSION};

/// 加载结果。
pub struct LoadOutcome {
    pub settings: SearchSettings,
    /// 给用户的通知（损坏隔离、未知版本等）。
    pub notice: Option<String>,
    /// 未知更高版本：只读，不回写（§6.2）。
    pub read_only: bool,
}

/// 设置文件存储（数据目录下的 `search/settings.json`）。
pub struct SearchSettingsStore {
    path: PathBuf,
}

impl SearchSettingsStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("settings.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 加载设置。文件不存在 → 默认；损坏 → 隔离为 `.corrupt.<unix_ts>` 后用默认；
    /// 未知更高版本 → 只读模式（不回写），并通知用户。
    pub fn load(&self) -> LoadOutcome {
        let Ok(bytes) = fs::read(&self.path) else {
            return LoadOutcome {
                settings: SearchSettings::default(),
                notice: None,
                read_only: false,
            };
        };
        // 先只取版本号，判断是否是未来版本。
        #[derive(serde::Deserialize)]
        struct VersionProbe {
            #[serde(default)]
            version: u32,
        }
        let probe: Option<VersionProbe> = serde_json::from_slice(&bytes).ok();
        match probe {
            Some(probe) if probe.version > SETTINGS_VERSION => LoadOutcome {
                settings: SearchSettings::default(),
                notice: Some(format!(
                    "搜索设置版本 {} 高于当前支持的 {}，已按只读默认设置运行（不会回写原文件）",
                    probe.version, SETTINGS_VERSION
                )),
                read_only: true,
            },
            _ => match serde_json::from_slice::<SearchSettings>(&bytes) {
                Ok(settings) => LoadOutcome {
                    settings,
                    notice: None,
                    read_only: false,
                },
                Err(err) => {
                    // 损坏隔离：重命名为 .corrupt.<unix_ts>（§6.2）。
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let quarantined = self.path.with_extension(format!("corrupt.{ts}"));
                    let renamed = fs::rename(&self.path, &quarantined).is_ok();
                    LoadOutcome {
                        settings: SearchSettings::default(),
                        notice: Some(format!(
                            "搜索设置已损坏（{err}），{}",
                            if renamed {
                                format!("已隔离为 {}，使用默认设置", quarantined.display())
                            } else {
                                "使用默认设置".to_string()
                            }
                        )),
                        read_only: false,
                    }
                }
            },
        }
    }

    /// 原子保存：先校验；写临时文件 → fsync → 原子替换（§6.2）。
    pub fn save(&self, settings: &SearchSettings) -> Result<(), String> {
        let problems = settings.validate();
        if !problems.is_empty() {
            return Err(problems.join("；"));
        }
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
            io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp, &self.path).map_err(|e| format!("保存搜索设置失败：{e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = SearchSettingsStore::new(dir.path());
        let mut settings = SearchSettings::default();
        settings.debounce_ms = 150;
        settings.ui.window.mode = super::super::model::SearchWindowMode::Pinned;
        store.save(&settings).unwrap();
        let loaded = store.load();
        assert!(!loaded.read_only);
        assert!(loaded.notice.is_none());
        assert_eq!(loaded.settings, settings);
    }

    #[test]
    fn corrupt_file_is_quarantined_with_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let store = SearchSettingsStore::new(dir.path());
        fs::write(store.path(), "{oops").unwrap();
        let loaded = store.load();
        assert!(loaded.notice.as_deref().unwrap_or("").contains("损坏"));
        // 隔离文件带时间戳后缀。
        let quarantined: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".corrupt."))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(loaded.settings, SearchSettings::default());
    }

    #[test]
    fn unknown_future_version_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = SearchSettingsStore::new(dir.path());
        fs::write(store.path(), r#"{"version":99,"debounce_ms":10}"#).unwrap();
        let loaded = store.load();
        assert!(loaded.read_only);
        assert!(loaded.notice.as_deref().unwrap_or("").contains("只读"));
        // 原文件未被改写。
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("99"));
    }

    #[test]
    fn save_rejects_invalid_settings() {
        let dir = tempfile::tempdir().unwrap();
        let store = SearchSettingsStore::new(dir.path());
        let mut settings = SearchSettings::default();
        settings.debounce_ms = 5000;
        assert!(store.save(&settings).is_err());
        assert!(!store.path().exists());
    }
}
