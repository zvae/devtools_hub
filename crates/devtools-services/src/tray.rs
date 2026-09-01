use std::{thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

/// 托盘线程发给应用运行时的事件。
#[derive(Clone, Debug)]
pub enum TrayEvent {
    ShowWindow,
    ShowClipboard { position: TrayPosition },
    Exit,
}

/// 托盘图标所在的屏幕物理坐标。
#[derive(Clone, Copy, Debug)]
pub struct TrayPosition {
    pub x: i32,
    pub y: i32,
}

/// 创建系统托盘图标、菜单，并监听左键点击和菜单动作。
pub fn spawn_tray_listener(tx: mpsc::UnboundedSender<TrayEvent>) -> Result<()> {
    let menu = Menu::new();
    let show_item = MenuItem::new("Show DevTools Hub", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append_items(&[&show_item, &PredefinedMenuItem::separator(), &quit_item])?;

    let show_id = show_item.id().clone();
    let quit_id = quit_item.id().clone();
    let tray = TrayIconBuilder::new()
        .with_tooltip("DevTools Hub")
        .with_icon(create_icon()?)
        .with_icon_as_template(cfg!(target_os = "macos"))
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    thread::Builder::new()
        .name("tray-listener".into())
        .spawn(move || {
            let receiver = MenuEvent::receiver();
            let tray_receiver = TrayIconEvent::receiver();
            loop {
                match tray_receiver.try_recv() {
                    Ok(TrayIconEvent::Click {
                        position,
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    })
                    | Ok(TrayIconEvent::DoubleClick {
                        position,
                        button: MouseButton::Left,
                        ..
                    }) => {
                        if tx
                            .send(TrayEvent::ShowClipboard {
                                position: TrayPosition {
                                    x: position.x.round() as i32,
                                    y: position.y.round() as i32,
                                },
                            })
                            .is_err()
                        {
                            debug!("tray listener stopped because receiver was dropped");
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        warn!("tray icon event channel disconnected");
                        return;
                    }
                }

                match receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(event) if event.id == show_id => {
                        if tx.send(TrayEvent::ShowWindow).is_err() {
                            debug!("tray listener stopped because receiver was dropped");
                            return;
                        }
                    }
                    Ok(event) if event.id == quit_id => {
                        let _ = tx.send(TrayEvent::Exit);
                        return;
                    }
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(error) => {
                        warn!(?error, "tray listener failed");
                        return;
                    }
                }
            }
        })?;

    // 托盘对象需要一直存活，否则系统托盘图标会被释放。
    Box::leak(Box::new(tray));
    Ok(())
}

/// 生成适合系统托盘尺寸的代码括号图标。
fn create_icon() -> Result<Icon> {
    let size = 48u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let color = if cfg!(target_os = "macos") {
        [255, 255, 255]
    } else {
        [94, 224, 182]
    };

    for y in 0..size {
        for x in 0..size {
            let px = (x as f32 + 0.5) / size as f32;
            let py = (y as f32 + 0.5) / size as f32;
            let mark = [
                ((0.38, 0.28), (0.24, 0.50)),
                ((0.24, 0.50), (0.38, 0.72)),
                ((0.62, 0.28), (0.76, 0.50)),
                ((0.76, 0.50), (0.62, 0.72)),
                ((0.58, 0.22), (0.42, 0.78)),
            ]
            .into_iter()
            .any(|(start, end)| point_near_segment(px, py, start, end, 0.085));

            rgba.extend_from_slice(&color);
            rgba.push(if mark { 255 } else { 0 });
        }
    }

    Ok(Icon::from_rgba(rgba, size, size)?)
}

fn point_near_segment(px: f32, py: f32, start: (f32, f32), end: (f32, f32), radius: f32) -> bool {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let length_squared = dx * dx + dy * dy;
    let projection = (((px - start.0) * dx + (py - start.1) * dy) / length_squared).clamp(0.0, 1.0);
    let closest_x = start.0 + projection * dx;
    let closest_y = start.1 + projection * dy;
    let distance_x = px - closest_x;
    let distance_y = py - closest_y;
    distance_x * distance_x + distance_y * distance_y <= radius * radius
}
