//! 菜单栏 / 托盘图标（PasteNow 是 `LSUIElement` 应用，只在菜单栏显示 `NSStatusItem`）。
//!
//! 与 PasteNow 一致：左键点击在图标旁弹出/收起面板，右键显示菜单。
//! macOS 上图标是模板图像，由系统按菜单栏明暗着色；Windows 上按任务栏主题画成黑色或白色。

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
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon([0, 0, 0])?);
        #[cfg(not(target_os = "macos"))]
        let builder = {
            let light = crate::platform::taskbar_uses_light_theme();
            builder.with_icon(icon(if light { [0, 0, 0] } else { [255, 255, 255] })?)
        };
        let icon = builder.build()?;
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

/// 程序绘制一个剪贴板形状的单色图标。
fn icon(rgb: [u8; 3]) -> Result<Icon> {
    const SIZE: u32 = 44;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let s = SIZE as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // 板身：圆角矩形描边
            let body = rounded_rect_sdf(px, py, (s * 0.2, s * 0.16), (s * 0.8, s * 0.94), s * 0.1);
            let mut alpha = coverage(body.abs() - s * 0.045);
            // 夹子：实心圆角矩形
            alpha = alpha.max(coverage(rounded_rect_sdf(
                px,
                py,
                (s * 0.34, s * 0.06),
                (s * 0.66, s * 0.24),
                s * 0.05,
            )));
            // 内容横线
            for (top, right) in [(0.42, 0.66), (0.57, 0.66), (0.72, 0.52)] {
                let line = rounded_rect_sdf(
                    px,
                    py,
                    (s * 0.34, s * top),
                    (s * right, s * (top + 0.06)),
                    s * 0.03,
                );
                alpha = alpha.max(coverage(line));
            }
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], (alpha * 255.0) as u8]);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}

fn rounded_rect_sdf(px: f32, py: f32, min: (f32, f32), max: (f32, f32), radius: f32) -> f32 {
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let (hx, hy) = (
        (max.0 - min.0) / 2.0 - radius,
        (max.1 - min.1) / 2.0 - radius,
    );
    let (dx, dy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}
