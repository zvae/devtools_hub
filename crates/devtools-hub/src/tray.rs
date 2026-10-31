//! 菜单栏 / 托盘图标（PasteNow 是 `LSUIElement` 应用，只在菜单栏显示 `NSStatusItem`）。
//!
//! 与 PasteNow 一致：左键点击在图标旁弹出/收起面板，右键显示菜单。
//! 图标是彩色的，菜单栏 / 任务栏明暗主题下都按原色显示。
//! 发现新版本时，菜单最上面多出一项“发现新版本”，点击打开设置页底部的更新信息。
//! 菜单里还有截屏、贴图与“关闭所有贴图”（对应 Snipaste 托盘菜单里的同名项）。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::controller;
use crate::hotkey::{self, HotkeyAction};
use crate::platform::TrayRect;
use crate::settings::Settings;
use crate::shared::{AppEvent, SearchEntry, Shared};

/// macOS 在按下时响应（与系统菜单栏一致）；Windows 在松开时响应，
/// 否则松开鼠标时任务栏会抢走焦点，刚弹出的面板又被隐藏。
const CLICK_STATE: MouseButtonState = if cfg!(windows) {
    MouseButtonState::Up
} else {
    MouseButtonState::Down
};

/// 从菜单里截屏时，等菜单淡出后再截，免得把菜单截进去（Snipaste 也有延时截屏 `startDelayed`）。
const SNIP_DELAY: Duration = Duration::from_millis(250);

/// 菜单栏（macOS）/ 托盘（Windows）图标与它的菜单。
pub struct Tray {
    _icon: TrayIcon,
    menu: Menu,
    pause: CheckMenuItem,
    show: MenuItem,
    search: MenuItem,
    snip: MenuItem,
    pin: MenuItem,
    update: MenuItem,
    update_separator: PredefinedMenuItem,
    /// 菜单里正在提示的新版本。
    update_version: Option<String>,
}

impl Tray {
    /// 必须在主线程、事件循环启动后创建。
    pub fn new(
        shared: Arc<Shared>,
        data_dir: std::path::PathBuf,
        settings: &Settings,
    ) -> Result<Self> {
        let show = MenuItem::with_id("show", label(settings, HotkeyAction::Panel), true, None);
        let search = MenuItem::with_id(
            "search",
            search_label(settings),
            settings.search_enabled,
            None,
        );
        let snip = MenuItem::with_id("snip", label(settings, HotkeyAction::Snip), true, None);
        let pin = MenuItem::with_id("pin", label(settings, HotkeyAction::Pin), true, None);
        let close_pins = MenuItem::with_id("close-pins", "关闭所有贴图", true, None);
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
        // 发现新版本时才插入菜单。
        let update = MenuItem::with_id("update", "发现新版本", true, None);
        let menu = Menu::with_items(&[
            &show,
            &search,
            &pause,
            &PredefinedMenuItem::separator(),
            &snip,
            &pin,
            &close_pins,
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
            "search" => shared.post(AppEvent::ShowSearch(SearchEntry::Hotkey)),
            "pause" => {
                shared.paused.fetch_xor(true, Ordering::Relaxed);
                shared.post(AppEvent::PauseChanged);
            }
            "settings" => controller::show_panel(&shared, true),
            "snip" => {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(SNIP_DELAY);
                    shared.post(AppEvent::Snip);
                });
            }
            "pin" => shared.post(AppEvent::PinClipboard),
            "close-pins" => shared.post(AppEvent::ClosePins),
            "data" => crate::platform::workspace::open_path(&data_dir),
            "update" => {
                controller::show_panel(&shared, true);
                shared.post(AppEvent::RevealUpdate);
            }
            // 正常关闭：交由 UI 线程收起窗口并走 on_exit（不再 process::exit）。
            // 兜底看门狗：UI 事件循环偶尔会卡到收不到重绘唤醒，正常关闭 5 秒没完成
            // 就强制结束进程，保证托盘退出永远可用。正常退出时进程先结束，看门狗随之消失。
            "quit" => {
                tracing::info!("托盘菜单：退出");
                crate::shared::quit_trace("1 tray: quit clicked");
                // 看门狗：UI 事件循环偶尔会卡到收不到重绘唤醒（egui/eframe 0.36 上游竞态，
                // 隐藏窗口的唤醒请求被静默丢弃），Quit 事件入队后无人消费。轮询消费标志，
                // 正常退出（一帧内消费）后本线程静默结束；3 秒仍未消费才强杀，保证托盘
                // 退出永远可用。on_exit 最长 1.5 秒的收尾发生在消费之后，不受影响。
                let watchdog_shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    for _ in 0..30 {
                        if watchdog_shared.quit_consumed.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    tracing::warn!("正常退出 3 秒未完成，强制结束进程");
                    crate::shared::quit_trace("W watchdog: firing process::exit");
                    std::process::exit(0);
                });
                shared.post(AppEvent::Quit)
            }
            // 面板的右键菜单（见 `ui::native_menu`）：交给界面在下一帧处理。
            id => shared.post(AppEvent::MenuCommand(id.to_string())),
        }));

        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
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
            menu,
            pause,
            show,
            search,
            snip,
            pin,
            update,
            update_separator: PredefinedMenuItem::separator(),
            update_version: None,
        })
    }

    /// 设置变化后同步菜单：“暂停记录”的勾选状态与快捷键提示。
    pub fn sync(&self, settings: &Settings) {
        self.pause.set_checked(settings.paused);
        self.show.set_text(label(settings, HotkeyAction::Panel));
        self.search.set_text(search_label(settings));
        // 未启用时置灰并提示（§4.3：托盘层展示"文件搜索（未启用）"）。
        self.search.set_enabled(settings.search_enabled);
        self.snip.set_text(label(settings, HotkeyAction::Snip));
        self.pin.set_text(label(settings, HotkeyAction::Pin));
    }

    /// 有需要提醒的新版本时在菜单最上面显示“发现新版本”，没有时移除。
    pub fn set_update(&mut self, version: Option<&str>) {
        if self.update_version.as_deref() == version {
            return;
        }
        let result = match version {
            Some(version) => {
                self.update.set_text(format!("发现新版本 v{version}…"));
                if self.update_version.is_some() {
                    Ok(())
                } else {
                    self.menu
                        .insert_items(&[&self.update, &self.update_separator], 0)
                }
            }
            None => self
                .menu
                .remove(&self.update)
                .and_then(|()| self.menu.remove(&self.update_separator)),
        };
        if let Err(err) = result {
            tracing::warn!("更新菜单栏菜单失败：{err}");
        }
        self.update_version = version.map(str::to_owned);
    }
}

/// 文件搜索菜单项：未启用时显示"文件搜索（未启用）"。
fn search_label(settings: &Settings) -> String {
    if !settings.search_enabled {
        return "文件搜索（未启用）".to_string();
    }
    let name = "文件搜索";
    let spec = settings.hotkey(HotkeyAction::Search);
    if spec.trim().is_empty() {
        name.to_string()
    } else {
        format!("{name}    {}", hotkey::display(spec))
    }
}

/// 菜单项的文字，后面附上快捷键提示（没有设置快捷键时不附）。
fn label(settings: &Settings, action: HotkeyAction) -> String {
    let name = match action {
        HotkeyAction::Panel => "打开剪贴板历史",
        HotkeyAction::Snip => "截屏",
        HotkeyAction::Pin => "贴图（剪贴板中的图片）",
        // 文件搜索菜单项有单独的文字（含"未启用"提示），不会走到这里。
        HotkeyAction::Search => "文件搜索",
    };
    let spec = settings.hotkey(action);
    if spec.trim().is_empty() {
        name.to_string()
    } else {
        format!("{name}    {}", hotkey::display(spec))
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
