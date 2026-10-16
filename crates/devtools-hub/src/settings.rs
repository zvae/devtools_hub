//! 用户设置与数据目录。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use devtools_clipboard::CaptureRules;
use serde::{Deserialize, Serialize};

use crate::platform::panel::Placement;
use crate::tools::json::Indent;

const SETTINGS_FILE: &str = "settings.json";
pub const DATA_DIR_ENV: &str = "DEVTOOLS_HUB_DATA_DIR";

/// 默认唤起快捷键。
/// macOS 与 PasteNow 一致（偏好里 carbonKeyCode = 9, carbonModifiers = cmdKey|shiftKey）；
/// Windows 上 Win+V 是系统剪贴板历史、Ctrl+Shift+V 常被用作“粘贴为纯文本”，因此用 Alt+Shift+V。
#[cfg(target_os = "macos")]
pub const DEFAULT_HOTKEY: &str = "Cmd+Shift+V";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_HOTKEY: &str = "Alt+Shift+V";

/// 数据目录：默认 `~/Library/Application Support/DevToolsHub`，
/// 可以用环境变量 `DEVTOOLS_HUB_DATA_DIR` 或 `--data-dir` 覆盖（开发调试时不污染真实数据）。
pub fn resolve_data_dir(cli: Option<PathBuf>) -> Result<PathBuf> {
    let dir = match cli.or_else(|| std::env::var_os(DATA_DIR_ENV).map(PathBuf::from)) {
        Some(dir) => dir,
        None => directories::BaseDirs::new()
            .context("找不到用户数据目录")?
            // macOS：~/Library/Application Support；Windows：%LOCALAPPDATA%（剪贴板数据不随漫游配置同步）。
            .data_local_dir()
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

/// 面板顶部的分类标签。“全部”固定在最前面，不在其中。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    Text,
    Link,
    Image,
    Files,
    /// 列表（收藏或用户新建的列表），值是列表 id。
    List(i64),
}

impl Tab {
    pub const BUILT_IN: [Tab; 4] = [Tab::Text, Tab::Link, Tab::Image, Tab::Files];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSetting {
    pub tab: Tab,
    pub visible: bool,
}

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
    /// 在其他应用中按下鼠标中键时弹出快捷面板（对应 uTools 的“鼠标按键弹出：按下鼠标中键”）。
    pub middle_click: bool,
    /// JSON 编辑器窗口大小（逻辑像素）。
    pub json_editor_size: [f32; 2],
    pub json_indent: Indent,
    /// 面板底色的透明度（0–100）。窗口背景是液态玻璃 / 毛玻璃时生效，越大越能透出背后的内容。
    pub panel_transparency: u8,
    /// 自动检查 GitHub Releases 上的新版本（启动时和之后每天一次）。
    pub auto_check_updates: bool,
    /// 用户选择跳过的版本：不再自动提醒；发布了更新的版本时照常提醒。
    pub skipped_version: Option<String>,
    /// 分类标签的顺序与是否显示，完整的顺序见 [`Settings::ordered_tabs`]。
    pub tabs: Vec<TabSetting>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
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
            middle_click: true,
            json_editor_size: [760.0, 560.0],
            json_indent: Indent::default(),
            panel_transparency: 60,
            auto_check_updates: true,
            skipped_version: None,
            tabs: Vec::new(),
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

    /// 按设置排好的全部分类标签：去掉已删除的列表；设置里没有的标签（新建的列表）排在最后并显示。
    pub fn ordered_tabs(&self, list_ids: &[i64]) -> Vec<TabSetting> {
        let mut tabs: Vec<TabSetting> = Vec::new();
        let known = |tabs: &[TabSetting], tab: Tab| tabs.iter().any(|t| t.tab == tab);
        for setting in &self.tabs {
            let exists = match setting.tab {
                Tab::List(id) => list_ids.contains(&id),
                _ => true,
            };
            if exists && !known(&tabs, setting.tab) {
                tabs.push(*setting);
            }
        }
        let all = Tab::BUILT_IN
            .into_iter()
            .chain(list_ids.iter().map(|&id| Tab::List(id)));
        for tab in all {
            if !known(&tabs, tab) {
                tabs.push(TabSetting { tab, visible: true });
            }
        }
        tabs
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

    #[test]
    fn orders_tabs_and_follows_lists() {
        let hidden = |tab| TabSetting {
            tab,
            visible: false,
        };
        let shown = |tab| TabSetting { tab, visible: true };
        // 默认：内置标签在前，列表按创建顺序在后，都显示。
        let tabs = Settings::default().ordered_tabs(&[1, 7]);
        assert_eq!(
            tabs,
            [
                shown(Tab::Text),
                shown(Tab::Link),
                shown(Tab::Image),
                shown(Tab::Files),
                shown(Tab::List(1)),
                shown(Tab::List(7)),
            ]
        );
        // 保存的顺序优先；已删除的列表（3）去掉，新建的列表（9）补在最后。
        let settings = Settings {
            tabs: vec![
                shown(Tab::List(1)),
                hidden(Tab::Image),
                hidden(Tab::List(3)),
                shown(Tab::Text),
                shown(Tab::Link),
                shown(Tab::Files),
            ],
            ..Settings::default()
        };
        assert_eq!(
            settings.ordered_tabs(&[1, 9]),
            [
                shown(Tab::List(1)),
                hidden(Tab::Image),
                shown(Tab::Text),
                shown(Tab::Link),
                shown(Tab::Files),
                shown(Tab::List(9)),
            ]
        );
        // 序列化成可读的 JSON。
        let json = serde_json::to_string(&settings.tabs[..2]).unwrap();
        assert_eq!(
            json,
            r#"[{"tab":{"list":1},"visible":true},{"tab":"image","visible":false}]"#
        );
    }
}
