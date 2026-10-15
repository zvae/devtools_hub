//! 菜单栏图标（PasteNow 是 `LSUIElement` 应用，只在菜单栏显示 `NSStatusItem`）。
//!
//! 与 PasteNow 一致：左键点击在图标正下方弹出/收起面板，右键（或 ⌃点击之外的辅助点击）显示菜单。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::controller;
use crate::shared::{AppEvent, Shared};

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
                button_state: MouseButtonState::Down,
                ..
            } = event
            {
                controller::toggle_from_status_item(&click_shared);
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
            "show" => controller::toggle_from_status_item(&shared),
            "pause" => {
                shared.paused.fetch_xor(true, Ordering::Relaxed);
                shared.post(AppEvent::PauseChanged);
            }
            "settings" => controller::show_panel(&shared, true),
            "data" => crate::platform::workspace::open_path(&data_dir),
            "quit" => std::process::exit(0),
            _ => {}
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .with_icon_templated(icon()?)
            .with_tooltip("DevTools Hub 剪贴板历史")
            .build()?;
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

/// 程序绘制一个剪贴板形状的模板图标（单色 + alpha，系统会按明暗模式着色）。
fn icon() -> Result<Icon> {
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
            rgba[i..i + 4].copy_from_slice(&[0, 0, 0, (alpha * 255.0) as u8]);
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
