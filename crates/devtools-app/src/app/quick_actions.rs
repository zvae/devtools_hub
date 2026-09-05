use super::*;

/// 打开中键快捷动作窗口，动作列表会根据选中文本内容做简单推荐。
pub(super) fn open_quick_action_window(
    storage: Storage,
    selected_text: Option<String>,
    position: Option<ScreenPosition>,
    dark_mode: bool,
    language: String,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
) {
    // 快捷动作弹窗独立于工具窗口设置，任何时刻只能显示一个实例。
    hide_quick_action_windows();

    let text = selected_text.unwrap_or_default();
    let actions = quick_actions_for(&text, commands.as_ref(), &tool_usage, &language);
    let action_ids = Rc::new(RefCell::new(
        actions
            .iter()
            .map(|action| action.id.clone())
            .collect::<Vec<_>>(),
    ));

    let window = take_quick_action_window();
    window.set_selected_text(text.clone().into());
    window.set_actions(results_to_model(actions, &language));
    window.set_dark_mode(dark_mode);
    window.set_language(language.clone().into());
    if let Some(position) = position {
        set_quick_action_position(&window, position);
    }

    let activate_window = window.as_weak();
    let activate_language = language.clone();
    let activate_tx = request_tx.clone();
    window.on_activate_action(move |index| {
        let action_id = action_ids.borrow().get(index as usize).cloned();
        if let Some(action_id) = action_id {
            if action_id == "tool.clipboard.history" {
                open_clipboard_window(storage.clone(), dark_mode, activate_language.clone());
            } else {
                open_tool_window(
                    action_id.clone(),
                    devtools_tools::localized_title_for(&action_id, &activate_language),
                    text.clone(),
                    dark_mode,
                    activate_language.clone(),
                    activate_tx.clone(),
                    Vec::new(),
                );
            }
            if let Some(window) = activate_window.upgrade() {
                window.hide().ok();
            }
        }
    });

    window.show().ok();
    QUICK_WINDOWS.with(|windows| windows.borrow_mut().push(window));
}

/// 提前构造唯一的中键快捷窗口，减少首次显示时的组件创建开销。
pub(super) fn precreate_quick_action_window() {
    QUICK_WINDOWS.with(|windows| {
        let mut windows = windows.borrow_mut();
        if windows.is_empty() {
            windows.push(QuickActionWindow::new().expect("failed to create quick action window"));
        }
    });
}

/// 取出唯一的中键快捷窗口，防止同一时刻存在多个快捷动作弹窗。
pub(super) fn take_quick_action_window() -> QuickActionWindow {
    QUICK_WINDOWS.with(|windows| {
        let mut windows = windows.borrow_mut();
        let window = windows.pop().unwrap_or_else(|| {
            QuickActionWindow::new().expect("failed to create quick action window")
        });
        windows.clear();
        window
    })
}

/// 将快捷窗口放到中键位置右下方。Quartz 使用逻辑坐标，Windows/rdev 使用物理坐标。
pub(super) fn set_quick_action_position(window: &QuickActionWindow, position: ScreenPosition) {
    #[cfg(target_os = "macos")]
    window.window().set_position(slint::LogicalPosition::new(
        position.x as f32 + 12.0,
        position.y as f32 + 12.0,
    ));

    #[cfg(not(target_os = "macos"))]
    window.window().set_position(slint::PhysicalPosition::new(
        position.x.saturating_add(12),
        position.y.saturating_add(12),
    ));
}

/// 隐藏唯一的快捷动作窗口，保留实例以加快下一次中键显示。
pub(super) fn hide_quick_action_windows() {
    QUICK_WINDOWS.with(|windows| {
        for window in windows.borrow().iter() {
            if window.window().is_visible() {
                let _ = window.hide();
            }
        }
    });
}

/// 显示已有窗口，并在支持的平台上恢复、激活并置于最前面。
pub(super) fn show_window_in_foreground(window: &slint::Window) {
    let was_visible = window.is_visible();
    if !was_visible {
        window.show().ok();
    }

    #[cfg(target_os = "windows")]
    bring_window_to_front(window);

    #[cfg(not(target_os = "windows"))]
    if was_visible {
        // Slint 目前没有跨平台公开的窗口激活 API；重新显示可请求窗口管理器激活。
        window.hide().ok();
        window.show().ok();
    }

    window.request_redraw();
}

#[cfg(target_os = "windows")]
pub(super) fn bring_window_to_front(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            BringWindowToTop, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
        },
    };

    let window_handle = window.window_handle();
    let Ok(handle) = window_handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);

    // 当前操作由用户在本应用内触发，Windows 允许恢复并激活该窗口。
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// 隐藏托盘打开的临时剪贴板弹窗，普通剪贴板窗口不受影响。
pub(super) fn hide_clipboard_popups() {
    CLIPBOARD_WINDOWS.with(|windows| {
        for state in windows.borrow().iter() {
            if state.window.get_popup_mode() && state.window.window().is_visible() {
                let _ = state.window.hide();
            }
        }
    });
}

pub(super) fn hide_transient_windows() {
    hide_quick_action_windows();
    hide_clipboard_popups();
}

/// 重新加载剪贴板窗口列表，并同步更新行 ID 映射。
pub(super) fn refresh_clipboard_window(
    window: &ClipboardWindow,
    storage: &Storage,
    ids: &Rc<RefCell<Vec<String>>>,
    query: &str,
    language: &str,
) {
    let rows = storage.search_clipboard(query, 100).unwrap_or_default();
    window.set_item_count(clipboard_count(rows.len(), language).into());
    *ids.borrow_mut() = rows
        .iter()
        .map(|record| format!("clipboard:{}", record.id))
        .collect();
    window.set_rows(clipboard_model(rows, language));
}

/// 根据选中文本生成快捷动作。优先推荐保留在列表顶部，其余工具与主窗口保持一致。
pub(super) fn quick_actions_for(
    selected_text: &str,
    commands: &[CommandDescriptor],
    usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
    language: &str,
) -> Vec<SearchResult> {
    let mut ids = Vec::new();
    if selected_text.trim_start().starts_with('{') || selected_text.trim_start().starts_with('[') {
        ids.push("tool.json.format");
        ids.push("tool.json.minify");
        ids.push("tool.json.validate");
    }
    ids.push("tool.base64.encode");
    ids.push("tool.base64.decode");
    ids.push("tool.clipboard.history");

    let mut remaining = commands
        .iter()
        .filter(|command| command.id != "setting.theme")
        .cloned()
        .collect::<Vec<_>>();
    sort_tool_commands(&mut remaining, usage);

    let mut ordered = Vec::with_capacity(remaining.len());
    for id in ids {
        if let Some(index) = remaining.iter().position(|command| command.id == id) {
            ordered.push(remaining.remove(index));
        }
    }
    ordered.extend(remaining);

    ordered
        .into_iter()
        .map(|command| {
            let (title, subtitle) = devtools_tools::localized_command_text(&command, language);
            SearchResult {
                id: command.id,
                title,
                subtitle,
                source: search_source_from_command(&command.source),
                score: 1.0,
            }
        })
        .collect()
}

pub(super) fn search_source_from_command(
    source: &CommandSource,
) -> devtools_core::search::SearchSource {
    match source {
        CommandSource::BuiltInTool => devtools_core::search::SearchSource::BuiltInTool,
        CommandSource::Plugin => devtools_core::search::SearchSource::Plugin,
        CommandSource::Clipboard => devtools_core::search::SearchSource::Clipboard,
        CommandSource::History => devtools_core::search::SearchSource::History,
        CommandSource::Setting => devtools_core::search::SearchSource::Setting,
    }
}

#[cfg(test)]
mod quick_action_tests {
    use super::*;

    #[test]
    fn quick_actions_include_the_main_tool_list_after_recommendations() {
        let commands = devtools_tools::builtin_commands();
        let usage = Arc::new(Mutex::new(HashMap::new()));
        let actions = quick_actions_for("{\"key\": true}", &commands, &usage, "zh-CN");
        let ids = actions
            .iter()
            .map(|action| action.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            &ids[..3],
            ["tool.json.format", "tool.json.minify", "tool.json.validate"]
        );
        assert_eq!(
            ids.len(),
            commands
                .iter()
                .filter(|command| command.id != "setting.theme")
                .count()
        );
        assert!(ids.contains(&"tool.sql.format"));
        assert!(ids.contains(&"tool.clipboard.history"));
    }
}
