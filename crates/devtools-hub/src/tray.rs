//! 菜单栏 / 托盘图标（PasteNow 是 `LSUIElement` 应用，只在菜单栏显示 `NSStatusItem`）。
//!
//! 与 PasteNow 一致：左键点击在图标旁弹出/收起面板，右键显示菜单。
//! 图标是彩色的，菜单栏 / 任务栏明暗主题下都按原色显示。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::controller;
use crate::platform::TrayRect;
use crate::shared::{AppEvent, Shared};

/// macOS 在按下时响应（与系统菜单栏一致）；Windows 在松开时响应，
/// 否则松开鼠标时任务栏会抢走焦点，刚弹出的面板又被隐藏。
const CLICK_STATE: MouseButtonState = if cfg!(windows) {
    MouseButtonState::Up
} else {
    MouseButtonState::Down
};

pub struct Tray {
    _icon: TrayIcon,
    pause: CheckMenuItem,
    show: MenuItem,
}

impl Tray {
    /// 必须在主线程、事件循环启动后创建。
    pub fn new(
        shared: Arc<Shared>,
        data_dir: std::path::PathBuf,
        hotkey_label: &str,
    ) -> Result<Self> {
        let show = MenuItem::with_id(
            "show",
            format!("打开剪贴板历史    {hotkey_label}"),
            true,
            None,
        );
        let pause = CheckMenuItem::with_id(
            "pause",
            "暂停记录",
            true,
            shared.paused.load(Ordering::Relaxed),
            None,
        );
        let settings = MenuItem::with_id("settings", "设置…", true, None);
        let data = MenuItem::with_id("data", "打开数据目录", true, None);
        let quit = MenuItem::with_id("quit", "退出 DevTools Hub", true, None);
        let menu = Menu::with_items(&[
            &show,
            &pause,
            &PredefinedMenuItem::separator(),
            &settings,
            &data,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let click_shared = shared.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state,
                rect,
                ..
            } = event
            {
                if button_state == CLICK_STATE {
                    let tray = TrayRect {
                        x: rect.position.x,
                        y: rect.position.y,
                        width: f64::from(rect.size.width),
                        height: f64::from(rect.size.height),
                    };
                    controller::toggle_from_status_item(&click_shared, Some(tray));
                }
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
            "show" => controller::toggle_from_status_item(&shared, None),
            "pause" => {
                shared.paused.fetch_xor(true, Ordering::Relaxed);
                shared.post(AppEvent::PauseChanged);
            }
            "settings" => controller::show_panel(&shared, true),
            "data" => crate::platform::workspace::open_path(&data_dir),
            "quit" => std::process::exit(0),
            _ => {}
        }));

        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_tooltip("DevTools Hub 剪贴板历史");
        // macOS 菜单栏图标高 22pt，按 2 倍像素提供；Windows 按系统 DPI 下的小图标尺寸提供。
        #[cfg(target_os = "macos")]
        let size = 44;
        #[cfg(not(target_os = "macos"))]
        let size = crate::platform::small_icon_size();
        let icon = builder.with_icon(icon(size)?).build()?;
        Ok(Self {
            _icon: icon,
            pause,
            show,
        })
    }

    pub fn sync(&self, paused: bool, hotkey_label: &str) {
        self.pause.set_checked(paused);
        self.show
            .set_text(format!("打开剪贴板历史    {hotkey_label}"));
    }
}

/// 菜单栏 / 托盘图标，由 `scripts/generate-icons.py` 从 `assets/menu_bar.png` 生成。
const TRAY_PNG: &[u8] = include_bytes!("../assets/tray.png");

/// 解码托盘图标并缩放到 `size` 像素见方，由我们而不是系统缩放，小尺寸下更清晰。
fn icon(size: u32) -> Result<Icon> {
    use image::imageops::{self, FilterType};

    let image = image::load_from_memory(TRAY_PNG)?.into_rgba8();
    let image = imageops::resize(&image, size, size, FilterType::Lanczos3);
    Ok(Icon::from_rgba(image.into_raw(), size, size)?)
}
