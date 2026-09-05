use super::*;

/// 打开独立工具窗口。工具窗口继承当前主题和语言，并支持置顶。
pub(super) fn open_tool_window(
    tool_id: String,
    title: String,
    input: String,
    dark_mode: bool,
    language: String,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    history: Vec<ToolHistoryRecord>,
) {
    if SINGLE_TOOL_WINDOW.load(Ordering::Relaxed) {
        let has_existing = TOOL_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            if let Some(state) = windows
                .iter()
                .find(|state| state.window.get_tool_id() == tool_id)
            {
                show_window_in_foreground(&state.window.window());
                true
            } else {
                false
            }
        });
        if has_existing {
            return;
        }
    }

    let window = ToolWindow::new().expect("failed to create tool window");
    window.set_tool_id(tool_id.clone().into());
    window.set_title_text(title.into());
    window.set_input(input.into());
    window.set_output("".into());
    window.set_dark_mode(dark_mode);
    window.set_language(language.into());
    window.set_pinned(false);
    window.set_history_visible(false);
    window.set_history(tool_history_model(&history));
    let history_records = Rc::new(RefCell::new(history));

    // UI 只发送意图，Core 决定执行内置工具或隔离的插件命令。
    let run_tool_id = tool_id.clone();
    let run_tx = request_tx.clone();
    window.on_run(move |input| {
        let _ = run_tx.send(AppRequest::RunTool {
            tool_id: run_tool_id.clone(),
            input: input.to_string(),
        });
    });

    let select_window = window.as_weak();
    let select_history = Rc::clone(&history_records);
    window.on_select_history(move |index| {
        let record = select_history.borrow().get(index as usize).cloned();
        if let (Some(window), Some(record)) = (select_window.upgrade(), record) {
            window.set_input(record.input.into());
            window.set_output(record.output.into());
        }
    });

    let history_window = window.as_weak();
    window.on_toggle_history(move || {
        if let Some(window) = history_window.upgrade() {
            window.set_history_visible(!window.get_history_visible());
        }
    });

    let copy_window = window.as_weak();
    window.on_copy_output(move || {
        if let Some(window) = copy_window.upgrade() {
            let _ = set_clipboard_text(&window.get_output());
        }
    });

    let pin_window = window.as_weak();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            window.set_pinned(!window.get_pinned());
        }
    });

    window.show().ok();
    if !window.get_input().is_empty() {
        let _ = request_tx.send(AppRequest::RunTool {
            tool_id: tool_id.clone(),
            input: window.get_input().to_string(),
        });
    }
    TOOL_WINDOWS.with(|windows| {
        windows.borrow_mut().push(ToolWindowState {
            window,
            history: history_records,
        })
    });
}
