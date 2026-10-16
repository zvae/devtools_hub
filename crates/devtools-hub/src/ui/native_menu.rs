//! 原生右键菜单：macOS 上是 `NSMenu`（macOS 26 起系统把它画成液态玻璃，与 PasteNow 一致），
//! Windows 上是系统弹出菜单。菜单不受面板窗口大小限制，子菜单可以展开到窗口外面。
//!
//! 菜单按 [`Entry`] 描述生成；选中的菜单项经托盘的菜单事件回调变成
//! [`AppEvent::MenuCommand`](crate::shared::AppEvent::MenuCommand)，下一帧用 [`NativeMenu::take`]
//! 换回对应的操作。

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers, CMD_OR_CTRL};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

use crate::platform::panel as native_panel;

pub enum Entry<A> {
    Item {
        label: String,
        shortcut: Option<Accelerator>,
        action: A,
    },
    /// 带勾选标记的菜单项。
    Check {
        label: String,
        checked: bool,
        action: A,
    },
    Submenu {
        label: String,
        entries: Vec<Entry<A>>,
    },
    Separator,
}

impl<A> Entry<A> {
    pub fn item(label: impl Into<String>, action: A) -> Self {
        Entry::Item {
            label: label.into(),
            shortcut: None,
            action,
        }
    }

    /// 右侧显示的快捷键（只是提示，按键由面板自己处理）。
    pub fn shortcut(label: impl Into<String>, modifiers: Modifiers, key: Code, action: A) -> Self {
        Entry::Item {
            label: label.into(),
            shortcut: Some(Accelerator::new(modifiers, key)),
            action,
        }
    }

    #[cfg(test)]
    pub fn label(&self) -> Option<&str> {
        match self {
            Entry::Item { label, .. }
            | Entry::Check { label, .. }
            | Entry::Submenu { label, .. } => Some(label),
            Entry::Separator => None,
        }
    }
}

pub const NONE: Modifiers = Modifiers::empty();
/// ⌘（Windows 上是 Ctrl）。
pub const COMMAND: Modifiers = CMD_OR_CTRL;
pub const SHIFT: Modifiers = Modifiers::SHIFT;

/// 弹出过的菜单与菜单项对应的操作。
pub struct NativeMenu<A> {
    /// 菜单留到下一次弹出前，保证事件到达时菜单项还在。
    _menu: Menu,
    actions: Vec<(String, A)>,
}

impl<A> NativeMenu<A> {
    /// 在鼠标位置弹出菜单，菜单关闭后才返回。面板窗口不存在（比如测试中）时不弹出，返回 `None`。
    pub fn popup(entries: Vec<Entry<A>>) -> Option<Self> {
        if !native_panel::is_visible() {
            return None;
        }
        let mut actions = Vec::new();
        let build = || -> Result<Menu> {
            let items = build_items(entries, &mut actions)?;
            let menu = Menu::new();
            menu.append_items(&refs(&items))?;
            Ok(menu)
        };
        let menu = build()
            .inspect_err(|err| tracing::warn!("创建右键菜单失败：{err:#}"))
            .ok()?;
        native_panel::popup_menu(&menu);
        Some(Self {
            _menu: menu,
            actions,
        })
    }

    /// 取出菜单项 `id` 对应的操作。
    pub fn take(&mut self, id: &str) -> Option<A> {
        let index = self.actions.iter().position(|(item, _)| item == id)?;
        Some(self.actions.swap_remove(index).1)
    }
}

/// 菜单项 id 都带这个前缀，与托盘菜单的 id 区分开。
pub const ID_PREFIX: &str = "context-";

fn next_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{ID_PREFIX}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn build_items<A>(
    entries: Vec<Entry<A>>,
    actions: &mut Vec<(String, A)>,
) -> Result<Vec<Box<dyn IsMenuItem>>> {
    let mut items: Vec<Box<dyn IsMenuItem>> = Vec::new();
    for entry in entries {
        match entry {
            Entry::Item {
                label,
                shortcut,
                action,
            } => {
                let id = next_id();
                items.push(Box::new(MenuItem::with_id(
                    id.as_str(),
                    escape(&label),
                    true,
                    shortcut,
                )));
                actions.push((id, action));
            }
            Entry::Check {
                label,
                checked,
                action,
            } => {
                let id = next_id();
                items.push(Box::new(CheckMenuItem::with_id(
                    id.as_str(),
                    escape(&label),
                    true,
                    checked,
                    None,
                )));
                actions.push((id, action));
            }
            Entry::Submenu { label, entries } => {
                let children = build_items(entries, actions)?;
                items.push(Box::new(Submenu::with_items(
                    escape(&label),
                    true,
                    &refs(&children),
                )?));
            }
            Entry::Separator => items.push(Box::new(PredefinedMenuItem::separator())),
        }
    }
    Ok(items)
}

fn refs(items: &[Box<dyn IsMenuItem>]) -> Vec<&dyn IsMenuItem> {
    items.iter().map(|item| item.as_ref()).collect()
}

/// Windows 菜单把 `&` 当作助记符前缀，列表名里的 `&` 要写成 `&&`。
fn escape(label: &str) -> String {
    if cfg!(windows) {
        label.replace('&', "&&")
    } else {
        label.to_string()
    }
}
