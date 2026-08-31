use std::{thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
};

#[derive(Clone, Debug)]
pub enum TrayEvent {
    ShowWindow,
    Exit,
}

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
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    })
                    | Ok(TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }) => {
                        if tx.send(TrayEvent::ShowWindow).is_err() {
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

                match receiver.recv_timeout(Duration::from_millis(250)) {
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

    Box::leak(Box::new(tray));
    Ok(())
}

fn create_icon() -> Result<Icon> {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let in_circle = dx * dx + dy * dy <= 14 * 14;
            if in_circle {
                rgba.extend_from_slice(&[52, 211, 153, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Ok(Icon::from_rgba(rgba, size, size)?)
}
