//! 右键菜单：macOS 上是原生的 `NSMenu`（macOS 26 起系统把它画成液态玻璃，与 PasteNow 一致）；
//! Windows 的系统菜单样式与面板不搭，改用自绘的同款菜单（见 `platform::windows::menu`）。
//! 菜单都是独立的窗口，不受面板窗口大小限制，子菜单可以展开到窗口外面。
//!
//! 菜单按 [`Entry`] 描述生成，每个菜单项对应一个 id，用 [`NativeMenu::take`] 换回对应的操作：
//! macOS 上选中的菜单项经托盘的菜单事件回调变成
//! [`AppEvent::MenuCommand`](crate::shared::AppEvent::MenuCommand)，下一帧送来 id；
//! Windows 的自绘菜单关闭时就知道选中了哪一项，直接随 [`NativeMenu::popup`] 返回。

use std::sync::atomic::{AtomicU64, Ordering};

use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers, CMD_OR_CTRL};

use super::style::Palette;
use crate::keys;
use crate::platform::drawn_menu::{Colors, Node};
use crate::platform::{panel as native_panel, Backdrop};

/// 菜单内容的描述，`A` 是选中菜单项后要执行的操作。
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
    /// 没有快捷键的普通菜单项。
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

    /// 菜单项的文字（分隔线没有），测试里用来检查菜单内容。
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

/// 菜单项快捷键的修饰键（只用来显示，按键本身由面板处理）。
pub const NONE: Modifiers = Modifiers::empty();
/// ⌘（Windows 上是 Ctrl）。
pub const COMMAND: Modifiers = CMD_OR_CTRL;
pub const SHIFT: Modifiers = Modifiers::SHIFT;

/// 弹出过的菜单与菜单项对应的操作。
pub struct NativeMenu<A> {
    /// macOS：菜单留到下一次弹出前，保证事件到达时菜单项还在。
    #[cfg(not(windows))]
    _menu: tray_icon::menu::Menu,
    actions: Vec<(String, A)>,
}

impl<A> NativeMenu<A> {
    /// 在鼠标位置弹出菜单，菜单关闭后才返回。面板窗口不存在（比如测试中）时不弹出，返回 `None`。
    /// 第二项是 Windows 上选中的菜单项 id（macOS 的 id 随后经菜单事件送来）。
    pub fn popup(entries: Vec<Entry<A>>, ctx: &egui::Context) -> Option<(Self, Option<String>)> {
        if !native_panel::is_visible() {
            return None;
        }
        let mut actions = Vec::new();
        #[cfg(windows)]
        {
            let nodes = drawn_nodes(entries, &mut actions);
            let chosen = crate::platform::menu::popup(&nodes, &drawn_colors(ctx))
                .map(|index| actions[index].0.clone());
            Some((Self { actions }, chosen))
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            let menu = nsmenu::build(entries, &mut actions)
                .inspect_err(|err| tracing::warn!("创建右键菜单失败：{err:#}"))
                .ok()?;
            native_panel::popup_menu(&menu);
            Some((
                Self {
                    _menu: menu,
                    actions,
                },
                None,
            ))
        }
    }

    /// 同 [`Self::popup`]，菜单属于标题为 `title` 的窗口（贴图窗口的右键菜单）；找不到窗口时不弹出。
    pub fn popup_in(
        entries: Vec<Entry<A>>,
        ctx: &egui::Context,
        title: &str,
    ) -> Option<(Self, Option<String>)> {
        let mut actions = Vec::new();
        #[cfg(windows)]
        {
            let owner = crate::platform::floating::window(title);
            if owner.is_null() {
                return None;
            }
            let nodes = drawn_nodes(entries, &mut actions);
            let chosen = crate::platform::menu::popup_owned(owner, &nodes, &drawn_colors(ctx))
                .map(|index| actions[index].0.clone());
            Some((Self { actions }, chosen))
        }
        #[cfg(not(windows))]
        {
            let _ = ctx;
            let menu = nsmenu::build(entries, &mut actions)
                .inspect_err(|err| tracing::warn!("创建右键菜单失败：{err:#}"))
                .ok()?;
            crate::platform::floating::popup_menu(title, &menu);
            Some((
                Self {
                    _menu: menu,
                    actions,
                },
                None,
            ))
        }
    }

    /// 取出菜单项 `id` 对应的操作。
    pub fn take(&mut self, id: &str) -> Option<A> {
        let index = self.actions.iter().position(|(item, _)| item == id)?;
        Some(self.actions.swap_remove(index).1)
    }
}

/// 菜单项 id 都带这个前缀，与托盘菜单的 id 区分开。
pub const ID_PREFIX: &str = "context-";

/// 生成不重复的菜单项 id。
fn next_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{ID_PREFIX}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// macOS：把菜单描述转成 `NSMenu`（经 muda）。
#[cfg(not(windows))]
mod nsmenu {
    use anyhow::Result;
    use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

    use super::{next_id, Entry};

    /// 生成菜单，同时记下每个菜单项 id 对应的操作。
    pub fn build<A>(entries: Vec<Entry<A>>, actions: &mut Vec<(String, A)>) -> Result<Menu> {
        let items = build_items(entries, actions)?;
        let menu = Menu::new();
        menu.append_items(&refs(&items))?;
        Ok(menu)
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
                        label,
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
                        label,
                        true,
                        checked,
                        None,
                    )));
                    actions.push((id, action));
                }
                Entry::Submenu { label, entries } => {
                    let children = build_items(entries, actions)?;
                    items.push(Box::new(Submenu::with_items(
                        label,
                        true,
                        &refs(&children),
                    )?));
                }
                Entry::Separator => items.push(Box::new(PredefinedMenuItem::separator())),
            }
        }
        Ok(items)
    }

    /// `Menu::append_items` 需要引用的切片。
    fn refs(items: &[Box<dyn IsMenuItem>]) -> Vec<&dyn IsMenuItem> {
        items.iter().map(|item| item.as_ref()).collect()
    }
}

/// Windows：把菜单描述转成自绘菜单的内容。菜单项的 `id` 是它在 `actions` 里的下标。
#[cfg_attr(not(windows), allow(dead_code))]
fn drawn_nodes<A>(entries: Vec<Entry<A>>, actions: &mut Vec<(String, A)>) -> Vec<Node> {
    entries
        .into_iter()
        .map(|entry| match entry {
            Entry::Item {
                label,
                shortcut,
                action,
            } => {
                let id = actions.len();
                actions.push((next_id(), action));
                Node::Item {
                    label,
                    shortcut: shortcut.as_ref().map(shortcut_text),
                    checked: None,
                    id,
                }
            }
            Entry::Check {
                label,
                checked,
                action,
            } => {
                let id = actions.len();
                actions.push((next_id(), action));
                Node::Item {
                    label,
                    shortcut: None,
                    checked: Some(checked),
                    id,
                }
            }
            Entry::Submenu { label, entries } => Node::Submenu {
                label,
                children: drawn_nodes(entries, actions),
            },
            Entry::Separator => Node::Separator,
        })
        .collect()
}

/// 快捷键的写法，与面板上的按键提示一致（Windows 上是 `Ctrl+Enter`）。
#[cfg_attr(not(windows), allow(dead_code))]
fn shortcut_text(accelerator: &Accelerator) -> String {
    let modifiers = accelerator.modifiers();
    let mut parts = Vec::new();
    for (modifier, label) in [
        (COMMAND, keys::COMMAND),
        (Modifiers::ALT, keys::ALT),
        (SHIFT, keys::SHIFT),
    ] {
        if modifiers.contains(modifier) {
            parts.push(label.to_string());
        }
    }
    parts.push(match accelerator.key() {
        Code::Enter => keys::RETURN.to_string(),
        Code::Backspace | Code::Delete => keys::DELETE.to_string(),
        Code::Escape => keys::ESCAPE.to_string(),
        Code::Tab => keys::TAB.to_string(),
        Code::Space => keys::SPACE.to_string(),
        code => {
            let name = code.to_string();
            let short = name
                .strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"));
            short.unwrap_or(&name).to_string()
        }
    });
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    keys::shortcut(&parts)
}

/// 自绘菜单的配色：与面板上的 egui 弹窗一致（底板、阴影见 `style::install`），文字与高亮用面板配色。
#[cfg_attr(not(windows), allow(dead_code))]
fn drawn_colors(ctx: &egui::Context) -> Colors {
    let palette = Palette::new(ctx.theme() == egui::Theme::Dark, Backdrop::Opaque, 0);
    let visuals = &ctx.global_style().visuals;
    Colors {
        fill: visuals.window_fill.to_srgba_unmultiplied(),
        border: palette.border.to_srgba_unmultiplied(),
        text: palette.text.to_srgba_unmultiplied(),
        weak: palette.weak.to_srgba_unmultiplied(),
        accent: palette.accent.to_srgba_unmultiplied(),
        shadow: visuals.popup_shadow.color.to_srgba_unmultiplied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_entries_for_drawn_menu() {
        let entries = vec![
            Entry::shortcut("粘贴", NONE, Code::Enter, 'a'),
            Entry::shortcut("仅复制", COMMAND, Code::Enter, 'b'),
            Entry::Separator,
            Entry::Submenu {
                label: "添加到列表".into(),
                entries: vec![Entry::Check {
                    label: "收藏".into(),
                    checked: true,
                    action: 'c',
                }],
            },
            Entry::shortcut("预览", COMMAND, Code::KeyY, 'd'),
        ];
        let mut actions = Vec::new();
        let nodes = drawn_nodes(entries, &mut actions);
        let order: Vec<char> = actions.iter().map(|(_, action)| *action).collect();
        assert_eq!(order, ['a', 'b', 'c', 'd']);

        // 菜单项的 id 是操作在 `actions` 里的下标，子菜单里的接着编号。
        let item = |label: &str, shortcut: &[&str], id| Node::Item {
            label: label.into(),
            shortcut: Some(keys::shortcut(shortcut)),
            checked: None,
            id,
        };
        assert_eq!(
            nodes,
            [
                item("粘贴", &[keys::RETURN], 0),
                item("仅复制", &[keys::COMMAND, keys::RETURN], 1),
                Node::Separator,
                Node::Submenu {
                    label: "添加到列表".into(),
                    children: vec![Node::Item {
                        label: "收藏".into(),
                        shortcut: None,
                        checked: Some(true),
                        id: 2,
                    }],
                },
                item("预览", &[keys::COMMAND, "Y"], 3),
            ]
        );
    }
}
