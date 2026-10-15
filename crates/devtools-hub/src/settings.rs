//! 用户设置与数据目录。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use devtools_clipboard::CaptureRules;
use serde::{Deserialize, Serialize};

use crate::platform::panel::Placement;

const SETTINGS_FILE: &str = "settings.json";
pub const DATA_DIR_ENV: &str = "DEVTOOLS_HUB_DATA_DIR";

/// 数据目录：默认 `~/Library/Application Support/DevToolsHub`，
/// 可以用环境变量 `DEVTOOLS_HUB_DATA_DIR` 或 `--data-dir` 覆盖（开发调试时不污染真实数据）。
pub fn resolve_data_dir(cli: Option<PathBuf>) -> Result<PathBuf> {
    let dir = match cli.or_else(|| std::env::var_os(DATA_DIR_ENV).map(PathBuf::from)) {
        Some(dir) => dir,
        None => directories::BaseDirs::new()
            .context("找不到用户数据目录")?
            .data_dir()
            .join("DevToolsHub"),
    };
    fs::create_dir_all(&dir).with_context(|| format!("创建数据目录失败：{}", dir.display()))?;
    Ok(dir)
}

/// 历史保留时长（对应 PasteNow 的 “Remove History Items”）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Retention {
    OneDay,
    OneWeek,
    #[default]
    OneMonth,
    ThreeMonths,
    SixMonths,
    OneYear,
    Forever,
}

impl Retention {
    pub const ALL: [Retention; 7] = [
        Retention::OneDay,
        Retention::OneWeek,
        Retention::OneMonth,
        Retention::ThreeMonths,
        Retention::SixMonths,
        Retention::OneYear,
        Retention::Forever,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Retention::OneDay => "1 天",
            Retention::OneWeek => "1 周",
            Retention::OneMonth => "1 个月",
            Retention::ThreeMonths => "3 个月",
            Retention::SixMonths => "6 个月",
            Retention::OneYear => "1 年",
            Retention::Forever => "永久（手动清理）",
        }
    }

    pub fn millis(self) -> Option<i64> {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        match self {
            Retention::OneDay => Some(DAY),
            Retention::OneWeek => Some(7 * DAY),
            Retention::OneMonth => Some(30 * DAY),
            Retention::ThreeMonths => Some(90 * DAY),
            Retention::SixMonths => Some(180 * DAY),
            Retention::OneYear => Some(365 * DAY),
            Retention::Forever => None,
        }
    }
}

/// 剪贴板轮询间隔（PasteNow 提供 0.1 / 0.5 / 1 秒三档）。
pub const POLL_INTERVALS: [(u64, &str); 4] = [
    (100, "0.1 秒（非常频繁）"),
    (250, "0.25 秒"),
    (500, "0.5 秒（推荐）"),
    (1000, "1 秒（省电）"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 唤起面板的全局快捷键，格式如 `Cmd+Shift+V`。
    pub hotkey: String,
    pub poll_interval_ms: u64,
    /// 暂停记录（菜单栏“暂停记录”）。
    pub paused: bool,
    pub retention: Retention,
    /// 历史最多保留条数，0 表示不限制（列表中的条目不计入）。
    pub max_items: u32,
    pub rules: CaptureRules,
    /// 选中后直接粘贴到之前的前台应用；关闭则只复制到剪贴板。
    pub paste_to_active_app: bool,
    pub move_to_top_after_paste: bool,
    pub placement: Placement,
    pub hide_on_blur: bool,
    /// 面板大小（逻辑像素），PasteNow 的默认大小约为 360×787。
    pub window_size: [f32; 2],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // 与 PasteNow 默认值一致（偏好里 carbonKeyCode = 9, carbonModifiers = cmdKey|shiftKey）。
            hotkey: "Cmd+Shift+V".to_string(),
            poll_interval_ms: 500,
            paused: false,
            retention: Retention::default(),
            max_items: 5000,
            rules: CaptureRules::default(),
            paste_to_active_app: true,
            move_to_top_after_paste: true,
            placement: Placement::default(),
            hide_on_blur: true,
            window_size: [380.0, 640.0],
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(SETTINGS_FILE);
        let Ok(text) = fs::read_to_string(&path) else {
            return Settings::default();
        };
        match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::warn!("设置文件解析失败，使用默认设置：{err}");
                let _ = fs::rename(&path, dir.join("settings.invalid.json"));
                Settings::default()
            }
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(SETTINGS_FILE);
        let tmp = dir.join("settings.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("保存设置失败：{}", path.display()))
    }

    pub fn max_items(&self) -> Option<usize> {
        (self.max_items > 0).then_some(self.max_items as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            hotkey: "Ctrl+Alt+V".into(),
            retention: Retention::Forever,
            placement: Placement::LastPosition,
            ..Settings::default()
        };
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), settings);
    }

    #[test]
    fn missing_fields_use_defaults_and_bad_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), r#"{"hotkey":"Cmd+B"}"#).unwrap();
        let settings = Settings::load(dir.path());
        assert_eq!(settings.hotkey, "Cmd+B");
        assert_eq!(settings.poll_interval_ms, 500);

        fs::write(dir.path().join(SETTINGS_FILE), "{oops").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        assert!(dir.path().join("settings.invalid.json").exists());
    }
}
