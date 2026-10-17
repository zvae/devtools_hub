//! 条目与分类标签的右键菜单（用原生菜单弹出，见 `native_menu`）。

use devtools_clipboard::{ClipItem, ContentKind};
use tray_icon::menu::accelerator::Code;

use super::Action;
use crate::platform::{self, panel as native_panel};
use crate::settings::Tab;
use crate::ui::native_menu::{self, Entry};
use crate::ui::{HubApp, PasteMode};

impl HubApp {
    /// 条目的右键菜单，参照 PasteNow：只有文字，右侧是快捷键。
    pub(in crate::ui) fn item_menu(&self, item: &ClipItem) -> Vec<Entry<Action>> {
        let id = item.id;
        let target = native_panel::previous_app_name()
            .map_or_else(|| "粘贴".to_string(), |app| format!("粘贴到“{app}”"));
        let mut lists: Vec<Entry<Action>> = self
            .panel
            .lists
            .iter()
            .map(|list| {
                let member = item.in_list(list.id);
                Entry::Check {
                    label: list.name.clone(),
                    checked: member,
                    action: Action::SetInList {
                        item: id,
                        list: list.id,
                        member: !member,
                    },
                }
            })
            .collect();
        lists.push(Entry::Separator);
        lists.push(Entry::item(
            "新建列表…",
            Action::NewList { add_item: Some(id) },
        ));

        let mut entries = vec![
            Entry::shortcut(
                target,
                native_menu::NONE,
                Code::Enter,
                Action::Paste(id, PasteMode::Original),
            ),
            Entry::shortcut(
                "以纯文本粘贴",
                native_menu::SHIFT,
                Code::Enter,
                Action::Paste(id, PasteMode::PlainText),
            ),
            Entry::shortcut(
                "仅复制",
                native_menu::COMMAND,
                Code::Enter,
                Action::Paste(id, PasteMode::CopyOnly),
            ),
            Entry::Separator,
            Entry::Submenu {
                label: "添加到列表".into(),
                entries: lists,
            },
        ];
        if item.kind == ContentKind::Files {
            let first_path = self
                .store
                .text(id)
                .ok()
                .flatten()
                .and_then(|text| text.lines().next().map(str::to_string));
            if let Some(path) = first_path {
                let label = format!("在{}中显示", platform::FILE_MANAGER);
                entries.push(Entry::item(label, Action::Reveal(path)));
            }
        }
        entries.extend([
            Entry::Separator,
            Entry::shortcut(
                "删除",
                native_menu::COMMAND,
                Code::Backspace,
                Action::Delete(id),
            ),
            Entry::Separator,
            Entry::shortcut(
                "预览",
                native_menu::COMMAND,
                Code::KeyY,
                Action::TogglePreview,
            ),
        ]);
        entries
    }
}

/// 分类标签的右键菜单。
pub(in crate::ui) fn tab_menu(tab: Option<Tab>) -> Vec<Entry<Action>> {
    let mut entries = Vec::new();
    if let Some(tab) = tab {
        entries.push(Entry::item("隐藏此标签", Action::HideTab(tab)));
    }
    entries.push(Entry::item("编辑分类标签…", Action::EditTabs));
    entries
}
