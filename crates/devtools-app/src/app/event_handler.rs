use super::*;

/// 后台运行时事件必须切回 Slint 事件循环线程处理。
pub(super) fn bind_runtime_events(
    app_weak: slint::Weak<SearchWindow>,
    search_ids: Arc<Mutex<Vec<String>>>,
    tool_ids: Arc<Mutex<Vec<String>>>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    mut event_rx: mpsc::UnboundedReceiver<AppEvent>,
) {
    std::thread::Builder::new()
        .name("ui-event-bridge".into())
        .spawn(move || {
            let runtime = Runtime::new().expect("failed to create ui event bridge runtime");
            runtime.block_on(async move {
                while let Some(event) = event_rx.recv().await {
                    let app_weak = app_weak.clone();
                    let search_ids = Arc::clone(&search_ids);
                    let tool_ids = Arc::clone(&tool_ids);
                    let commands = Arc::clone(&commands);
                    let tool_usage = Arc::clone(&tool_usage);
                    let request_tx = request_tx.clone();
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            apply_event(
                                &app,
                                &search_ids,
                                &tool_ids,
                                &commands,
                                &tool_usage,
                                request_tx,
                                event,
                            );
                        }
                    }) {
                        error!(?error, "failed to dispatch UI event");
                        return;
                    }
                }
            });
        })
        .expect("failed to spawn ui event bridge");
}

/// 将核心事件应用到主窗口状态，所有 UI 属性更新集中在这里。
pub(super) fn apply_event(
    app: &SearchWindow,
    search_ids: &Arc<Mutex<Vec<String>>>,
    tool_ids: &Arc<Mutex<Vec<String>>>,
    commands: &Arc<Vec<CommandDescriptor>>,
    tool_usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    event: AppEvent,
) {
    match event {
        AppEvent::SearchCompleted(results) => {
            set_ids(search_ids, results.iter().map(|result| result.id.clone()));
            let language = app.get_language().to_string();
            app.set_results(results_to_model(results, &language));
        }
        AppEvent::ClipboardStored => {
            app.set_status(localized_status(app, "剪贴板已保存", "Clipboard saved"));
        }
        AppEvent::SettingsLoaded {
            theme,
            language,
            hotkey,
            autostart,
            middle_click_enabled,
            tool_history_limit,
            single_tool_window,
        } => {
            apply_theme(app, &theme);
            app.set_language(language.into());
            app.set_settings_hotkey(hotkey.into());
            app.set_settings_autostart(autostart);
            app.set_settings_middle_click_enabled(middle_click_enabled);
            app.set_settings_tool_history_limit(tool_history_limit.to_string().into());
            app.set_settings_single_tool_window(single_tool_window);
            SINGLE_TOOL_WINDOW.store(single_tool_window, Ordering::Relaxed);
            app.set_active_view(2);
            app.set_status("".into());
        }
        AppEvent::ThemeChanged { theme } => {
            apply_theme(app, &theme);
            app.set_status(localized_status(app, "主题已更新", "Theme updated"));
        }
        AppEvent::LanguageChanged { language } => {
            app.set_language(language.into());
            app.set_status(localized_status(app, "语言已更新", "Language updated"));
        }
        AppEvent::HotkeyChanged { hotkey } => {
            app.set_settings_hotkey(hotkey.into());
            app.set_status(localized_status(app, "快捷键已保存", "Hotkey saved"));
        }
        AppEvent::AutostartChanged { enabled } => {
            app.set_settings_autostart(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已开启开机自启动"
                } else {
                    "已关闭开机自启动"
                },
                if enabled {
                    "Autostart enabled"
                } else {
                    "Autostart disabled"
                },
            ));
        }
        AppEvent::MiddleClickChanged { enabled } => {
            app.set_settings_middle_click_enabled(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已开启鼠标中键快捷弹窗"
                } else {
                    "已关闭鼠标中键快捷弹窗"
                },
                if enabled {
                    "Middle-click actions enabled"
                } else {
                    "Middle-click actions disabled"
                },
            ));
        }
        AppEvent::CommandExecuted {
            command_id,
            title,
            content,
        } => {
            if let Some(command_id) = command_id {
                record_tool_usage(tool_usage, &command_id);
                refresh_tool_list(app, tool_ids, commands, tool_usage);
            }
            app.set_status(format!("{title}: {content}").into());
        }
        AppEvent::ToolCompleted {
            tool_id,
            title,
            content,
        } => {
            record_tool_usage(tool_usage, &tool_id);
            TOOL_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    if state.window.get_tool_id() == tool_id {
                        state.window.set_output(content.clone().into());
                    }
                }
            });
            refresh_tool_list(app, tool_ids, commands, tool_usage);
            app.set_status(format!("{title}: {content}").into());
        }
        AppEvent::ToolHistoryUpdated { tool_id, history } => {
            TOOL_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    if state.window.get_tool_id() == tool_id {
                        state.window.set_history(tool_history_model(&history));
                        *state.history.borrow_mut() = history.clone();
                    }
                }
            });
        }
        AppEvent::CopyRequested { text } => {
            if let Err(error) = set_clipboard_text(&text) {
                app.set_status(format!("Copy failed: {error}").into());
            }
        }
        AppEvent::ToolOpened {
            tool_id,
            title,
            history,
        } => {
            let language = app.get_language().to_string();
            if tool_id == "tool.timestamp.convert" {
                open_timestamp_window(app.get_dark_mode(), request_tx);
            } else {
                open_tool_window(
                    tool_id.clone(),
                    localized_title(&tool_id, &language, &title),
                    String::new(),
                    app.get_dark_mode(),
                    language,
                    request_tx,
                    history,
                );
            }
        }
        AppEvent::ToggleWindow => {
            app.window().show().ok();
            app.window().request_redraw();
        }
        AppEvent::ExitRequested => {
            slint::quit_event_loop().ok();
        }
        AppEvent::Error(message) => {
            app.set_status(format!("Error: {message}").into());
        }
        AppEvent::ToolHistoryLimitChanged { limit } => {
            app.set_settings_tool_history_limit(limit.to_string().into());
            app.set_status(localized_status(
                app,
                "工具历史设置已保存",
                "Tool history setting saved",
            ));
        }
        AppEvent::SingleToolWindowChanged { enabled } => {
            SINGLE_TOOL_WINDOW.store(enabled, Ordering::Relaxed);
            app.set_settings_single_tool_window(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已启用工具单窗口"
                } else {
                    "已允许多个工具窗口"
                },
                if enabled {
                    "Single tool window enabled"
                } else {
                    "Multiple tool windows enabled"
                },
            ));
        }
        AppEvent::TimestampConverted { mode, value } => {
            TIMESTAMP_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    match mode {
                        TimestampConversionMode::UnixToDatetime => {
                            state.window.set_datetime_output(value.clone().into());
                        }
                        TimestampConversionMode::DatetimeToUnix => {
                            state
                                .window
                                .set_datetime_timestamp_output(value.clone().into());
                        }
                        TimestampConversionMode::PartsToUnix => {
                            state.window.set_parts_output(value.clone().into());
                        }
                    }
                }
            });
        }
    }
}
