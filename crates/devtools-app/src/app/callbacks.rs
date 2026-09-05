use super::*;

/// 绑定 Slint 回调。UI 只发送请求或打开窗口，耗时逻辑放到运行时/服务层。
#[allow(clippy::too_many_arguments)]
pub(super) fn bind_ui_callbacks(
    app: &SearchWindow,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    storage: Storage,
    search_ids: Arc<Mutex<Vec<String>>>,
    tool_ids: Arc<Mutex<Vec<String>>>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
    shortcut: Option<GlobalShortcut>,
    hotkey: String,
    middle_click: MiddleClickController,
) {
    let search_tx = request_tx.clone();
    // 搜索输入实时进入核心运行时，结果通过 AppEvent 异步回填。
    app.on_query_changed(move |query| {
        let _ = search_tx.send(AppRequest::Search {
            query: query.to_string(),
        });
    });

    let activate_app = app.as_weak();
    let activate_storage = storage.clone();
    let activate_tx = request_tx.clone();
    // 主搜索结果和工具列表共用激活逻辑，靠 ID 判断实际动作。
    app.on_activate_result(move |index| {
        if let Some(result_id) = get_id(&search_ids, index) {
            activate_item(
                &activate_app,
                activate_storage.clone(),
                activate_tx.clone(),
                &result_id,
                None,
            );
        }
    });

    let tools_app = app.as_weak();
    let tools_storage = storage.clone();
    let tools_tx = request_tx.clone();
    let activate_tool_ids = Arc::clone(&tool_ids);
    app.on_activate_tool(move |index| {
        if let Some(result_id) = get_id(&activate_tool_ids, index) {
            activate_item(
                &tools_app,
                tools_storage.clone(),
                tools_tx.clone(),
                &result_id,
                None,
            );
        }
    });

    let show_search_app = app.as_weak();
    let show_search_tx = request_tx.clone();
    app.on_show_search(move || {
        if let Some(app) = show_search_app.upgrade() {
            app.set_active_view(0);
            let _ = show_search_tx.send(AppRequest::Search {
                query: app.get_query().to_string(),
            });
        }
    });

    let show_tools_app = app.as_weak();
    let show_tools_commands = Arc::clone(&commands);
    let show_tools_usage = Arc::clone(&tool_usage);
    // tools 页面只展示工具类命令，并按当前语言生成列表文案。
    app.on_show_tools(move || {
        if let Some(app) = show_tools_app.upgrade() {
            let language = app.get_language().to_string();
            let mut commands = show_tools_commands
                .iter()
                .filter(|command| command.id != "setting.theme")
                .cloned()
                .collect::<Vec<_>>();
            sort_tool_commands(&mut commands, &show_tools_usage);
            set_ids(&tool_ids, commands.iter().map(|cmd| cmd.id.clone()));
            app.set_tools(command_model(commands, &language));
            app.set_active_view(1);
            app.set_status("".into());
        }
    });

    let settings_tx = request_tx.clone();
    app.on_show_settings(move || {
        let _ = settings_tx.send(AppRequest::OpenSettings);
    });

    let theme_tx = request_tx.clone();
    app.on_theme_selected(move |theme| {
        let _ = theme_tx.send(AppRequest::SetTheme {
            theme: theme.to_string(),
        });
    });

    let language_tx = request_tx.clone();
    let language_app = app.as_weak();
    let language_commands = Arc::clone(&commands);
    let language_usage = Arc::clone(&tool_usage);
    // 语言切换先立即更新 UI，再写入存储，减少界面反馈延迟。
    app.on_language_selected(move |language| {
        let language = language.to_string();
        if let Some(app) = language_app.upgrade() {
            app.set_language(language.clone().into());
            let mut commands = language_commands
                .iter()
                .filter(|command| command.id != "setting.theme")
                .cloned()
                .collect::<Vec<_>>();
            sort_tool_commands(&mut commands, &language_usage);
            app.set_tools(command_model(commands, &language));
            let _ = language_tx.send(AppRequest::Search {
                query: app.get_query().to_string(),
            });
        }
        let _ = language_tx.send(AppRequest::SetLanguage { language });
    });

    let hotkey_tx = request_tx.clone();
    let current_hotkey = Rc::new(RefCell::new(hotkey));
    let shortcut = Rc::new(RefCell::new(shortcut));
    let hotkey_app = app.as_weak();
    app.on_hotkey_changed(move |hotkey| {
        let hotkey = hotkey.to_string();
        let previous = current_hotkey.borrow().clone();
        let update_result = shortcut
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("global shortcut is unavailable"))
            .and_then(|shortcut| update_global_shortcut(shortcut, &hotkey));
        match update_result {
            Ok(()) => {
                *current_hotkey.borrow_mut() = hotkey.clone();
                let _ = hotkey_tx.send(AppRequest::SetHotkey { hotkey });
            }
            Err(error) => {
                if let Some(app) = hotkey_app.upgrade() {
                    app.set_settings_hotkey(previous.into());
                    app.set_status(format!("Shortcut unavailable: {error}").into());
                }
            }
        }
    });

    let autostart_tx = request_tx.clone();
    let autostart_app = app.as_weak();
    app.on_autostart_changed(move |enabled| match set_autostart_enabled(enabled) {
        Ok(()) => {
            let _ = autostart_tx.send(AppRequest::SetAutostart { enabled });
        }
        Err(error) => {
            if let Some(app) = autostart_app.upgrade() {
                app.set_settings_autostart(!enabled);
                app.set_status(format!("Autostart unavailable: {error}").into());
            }
        }
    });

    let middle_click_tx = request_tx.clone();
    let middle_click_app = app.as_weak();
    app.on_middle_click_enabled_changed(move |enabled| {
        middle_click.set_enabled(enabled);
        if let Err(error) = middle_click_tx.send(AppRequest::SetMiddleClickEnabled { enabled }) {
            middle_click.set_enabled(!enabled);
            if let Some(app) = middle_click_app.upgrade() {
                app.set_settings_middle_click_enabled(!enabled);
                app.set_status(format!("Middle-click unavailable: {error}").into());
            }
        }
    });

    let single_window_tx = request_tx.clone();
    app.on_single_tool_window_changed(move |enabled| {
        SINGLE_TOOL_WINDOW.store(enabled, Ordering::Relaxed);
        let _ = single_window_tx.send(AppRequest::SetSingleToolWindow { enabled });
    });

    let history_limit_tx = request_tx.clone();
    let history_limit_app = app.as_weak();
    app.on_tool_history_limit_changed(move |value| match value.parse::<usize>() {
        Ok(limit) if limit <= 1_000 => {
            let _ = history_limit_tx.send(AppRequest::SetToolHistoryLimit { limit });
        }
        _ => {
            if let Some(app) = history_limit_app.upgrade() {
                app.set_status(localized_status(
                    &app,
                    "历史条数需为 0 到 1000",
                    "History limit must be 0 to 1000",
                ));
            }
        }
    });

    app.on_clear_clipboard(move || {
        let _ = request_tx.send(AppRequest::ClearClipboard);
    });
}
