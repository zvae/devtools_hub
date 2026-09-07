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
                if !input.is_empty() {
                    state.window.set_input(input.clone().into());
                    let _ = request_tx.send(AppRequest::RunTool {
                        tool_id: tool_id.clone(),
                        input: input.clone(),
                    });
                }
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
    let json = Rc::new(RefCell::new(None));

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
    let select_json = Rc::clone(&json);
    window.on_select_history(move |index| {
        let record = select_history.borrow().get(index as usize).cloned();
        if let (Some(window), Some(record)) = (select_window.upgrade(), record) {
            window.set_input(record.input.into());
            update_tool_output(&window, &select_json, record.output);
        }
    });

    let fold_window = window.as_weak();
    let fold_json = Rc::clone(&json);
    window.on_toggle_json_fold(move |line| {
        if let (Some(window), Some(doc)) = (fold_window.upgrade(), fold_json.borrow_mut().as_mut())
        {
            if let Ok(line) = usize::try_from(line) {
                doc.toggle(line);
                doc.update_window(&window);
            }
        }
    });
    let fold_window = window.as_weak();
    let fold_json = Rc::clone(&json);
    window.on_fold_all_json(move |collapsed| {
        if let (Some(window), Some(doc)) = (fold_window.upgrade(), fold_json.borrow_mut().as_mut())
        {
            doc.set_all_collapsed(collapsed);
            doc.update_window(&window);
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

    if let Err(error) = window.show() {
        error!(?error, %tool_id, "failed to show tool window");
        return;
    }
    show_window_in_foreground(window.window());
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
            json,
        })
    });
}

pub(super) fn update_tool_output(
    window: &ToolWindow,
    json: &RefCell<Option<json_output::JsonDocument>>,
    output: String,
) {
    let document = json_output::JsonDocument::parse(&output);
    window.set_json_output(document.is_some());
    if let Some(document) = &document {
        document.update_window(window);
    } else {
        window.set_json_lines(ModelRc::default());
        window.set_json_display_text("".into());
    }
    *json.borrow_mut() = document;
    window.set_output(output.into());
}
