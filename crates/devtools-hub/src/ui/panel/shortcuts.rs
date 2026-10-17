//! 面板上的键盘快捷键。

use devtools_clipboard::FAVORITES_LIST_ID;
use egui::{Key, Modifiers};

use super::Action;
use crate::ui::{HubApp, PasteMode, NUMBER_KEYS};

impl HubApp {
    /// 读取并消费这一帧的快捷键：粘贴、删除、收藏、上下移动、切换分类等。
    /// 需要修改条目或离开面板的操作放进 `actions`，只改选中项、分类的直接改面板状态。
    pub(super) fn panel_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let panel = &mut self.panel;
        let selected = panel
            .selected_item()
            .map(|item| (item.id, item.in_list(FAVORITES_LIST_ID)));
        let count = panel.items.len();
        let query_empty = panel.query.is_empty();
        let mut quick_paste = None;
        let mut movement: Option<isize> = None;
        let (mut jump_first, mut jump_last, mut escape, mut tab, mut back_tab, mut preview) =
            (false, false, false, false, false, false);

        ctx.input_mut(|input| {
            // 顺序很重要：Modifiers::NONE 也会匹配按住 ⇧ 的按键，所以先检查带修饰键的组合。
            if let Some((id, favorite)) = selected {
                if input.consume_key(Modifiers::SHIFT, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::PlainText));
                } else if input.consume_key(Modifiers::COMMAND, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::CopyOnly));
                } else if input.consume_key(Modifiers::NONE, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::Original));
                }
                if input.consume_key(Modifiers::COMMAND, Key::Backspace)
                    || input.consume_key(Modifiers::COMMAND, Key::Delete)
                {
                    actions.push(Action::Delete(id));
                }
                if input.consume_key(Modifiers::COMMAND, Key::D) {
                    actions.push(Action::SetInList {
                        item: id,
                        list: FAVORITES_LIST_ID,
                        member: !favorite,
                    });
                }
                // 空格只在搜索框为空时用于预览，否则正常输入空格。
                preview = input.consume_key(Modifiers::COMMAND, Key::Y)
                    || (query_empty && input.consume_key(Modifiers::NONE, Key::Space));
            }
            if input.consume_key(Modifiers::COMMAND, Key::Comma) {
                actions.push(Action::OpenSettings);
            }
            for (index, key) in NUMBER_KEYS.iter().enumerate() {
                if input.consume_key(Modifiers::COMMAND, *key) {
                    quick_paste = Some(index);
                }
            }
            back_tab = input.consume_key(Modifiers::SHIFT, Key::Tab);
            tab = !back_tab && input.consume_key(Modifiers::NONE, Key::Tab);
            jump_first = input.consume_key(Modifiers::COMMAND, Key::ArrowUp);
            jump_last = input.consume_key(Modifiers::COMMAND, Key::ArrowDown);
            if input.consume_key(Modifiers::NONE, Key::ArrowUp)
                || input.consume_key(Modifiers::CTRL, Key::P)
            {
                movement = Some(-1);
            }
            if input.consume_key(Modifiers::NONE, Key::ArrowDown)
                || input.consume_key(Modifiers::CTRL, Key::N)
            {
                movement = Some(1);
            }
            if input.consume_key(Modifiers::NONE, Key::PageUp) {
                movement = Some(-5);
            }
            if input.consume_key(Modifiers::NONE, Key::PageDown) {
                movement = Some(5);
            }
            escape = input.consume_key(Modifiers::NONE, Key::Escape);
        });

        if preview {
            actions.push(Action::TogglePreview);
        }
        if let Some(item) = quick_paste.and_then(|index| panel.items.get(index)) {
            actions.push(Action::Paste(item.id, PasteMode::Original));
        }
        if tab || back_tab {
            panel.cycle_filter(tab, &self.settings);
        }
        if jump_first {
            panel.select(0);
        }
        if jump_last {
            panel.select(count.saturating_sub(1));
        }
        if let Some(delta) = movement {
            panel.select(panel.selected.saturating_add_signed(delta));
        }
        if escape {
            if panel.preview_open {
                panel.preview_open = false;
            } else if !panel.query.is_empty() {
                panel.query.clear();
                panel.selected = 0;
                panel.needs_reload = true;
            } else {
                actions.push(Action::Close);
            }
        }
    }
}
