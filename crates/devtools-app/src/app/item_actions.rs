use super::*;

/// 根据结果 ID 执行动作：设置和剪贴板由 UI 打开，其余命令统一交由 Core 执行。
pub(super) fn activate_item(
    app_weak: &slint::Weak<SearchWindow>,
    storage: Storage,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    result_id: &str,
    _input: Option<String>,
) {
    let dark_mode = app_weak
        .upgrade()
        .map(|app| app.get_dark_mode())
        .unwrap_or(true);
    let language = app_weak
        .upgrade()
        .map(|app| app.get_language().to_string())
        .unwrap_or_else(|| "zh-CN".into());

    match result_id {
        "setting.theme" => {
            let _ = request_tx.send(AppRequest::OpenSettings);
        }
        "tool.clipboard.history" => {
            open_clipboard_window(storage, dark_mode, language);
        }
        _ => {
            let _ = request_tx.send(AppRequest::ActivateResult {
                result_id: result_id.to_string(),
            });
        }
    }
}
