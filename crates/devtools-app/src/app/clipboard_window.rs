use super::*;

/// 打开剪贴板历史窗口，并维护 UI 行索引到数据库 ID 的映射。
pub(super) fn open_clipboard_window(storage: Storage, dark_mode: bool, language: String) {
    open_clipboard_window_internal(storage, dark_mode, language, false, false, None);
}

/// 托盘左键打开的临时剪贴板弹窗：无标题栏、跟随托盘位置并自动置顶。
pub(super) fn open_clipboard_popup(
    storage: Storage,
    dark_mode: bool,
    language: String,
    position: TrayPosition,
) {
    open_clipboard_window_internal(storage, dark_mode, language, true, false, Some(position));
}

/// 打开一个保留原生标题栏且已置顶的剪贴板窗口。
pub(super) fn open_pinned_clipboard_window(storage: Storage, dark_mode: bool, language: String) {
    open_clipboard_window_internal(storage, dark_mode, language, false, true, None);
}

/// 启动时预创建托盘剪贴板弹窗（保持隐藏）。
/// 窗口创建是首次托盘点击卡顿的主要来源，提前创建后点击时只需刷新数据并显示。
/// 主题/语言在每次显示前都会按当前设置刷新，预创建时使用默认值即可。
pub(super) fn precreate_clipboard_popup(storage: &Storage) {
    let state = build_clipboard_window(storage, true, "zh-CN", true, false);
    CLIPBOARD_WINDOWS.with(|windows| windows.borrow_mut().push(state));
}

pub(super) fn open_clipboard_window_internal(
    storage: Storage,
    dark_mode: bool,
    language: String,
    popup_mode: bool,
    pinned: bool,
    position: Option<TrayPosition>,
) {
    if popup_mode {
        if show_existing_clipboard_popup(
            &storage,
            dark_mode,
            &language,
            position.expect("clipboard popup position is required"),
        ) {
            return;
        }
    } else if show_existing_clipboard_window(&storage, dark_mode, &language, pinned) {
        return;
    }

    let state = build_clipboard_window(&storage, dark_mode, &language, popup_mode, pinned);

    // 先定位再显示，避免窗口先出现在系统默认位置再跳动到托盘附近。
    if let Some(position) = position {
        set_clipboard_popup_position(&state.window, position);
    }
    state.window.show().ok();
    CLIPBOARD_WINDOWS.with(|windows| windows.borrow_mut().push(state));
}

/// 创建剪贴板窗口并绑定全部回调，但不负责显示。
pub(super) fn build_clipboard_window(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    popup_mode: bool,
    pinned: bool,
) -> ClipboardWindowState {
    let window = ClipboardWindow::new().expect("failed to create clipboard window");
    let ids = Rc::new(RefCell::new(Vec::<String>::new()));
    window.set_popup_mode(popup_mode);
    window.set_pinned(pinned);
    window.set_dark_mode(dark_mode);
    window.set_language(language.into());
    // 隐藏状态下创建/显示的窗口不会触发 Slint show 时的 preferred 尺寸应用
    // (set_visible 走 Shown 路径被当作 recreating),会停在 min 尺寸 360x420,
    // 这里显式设为首选尺寸,保证窗口大小与托盘定位计算一致。
    window
        .window()
        .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
            420.0, 620.0,
        )));
    refresh_clipboard_window(&window, storage, &ids, "", language);

    let query_window = window.as_weak();
    let query_storage = storage.clone();
    let query_ids = Rc::clone(&ids);
    let query_language = language.to_string();
    window.on_query_changed(move |query| {
        if let Some(window) = query_window.upgrade() {
            refresh_clipboard_window(&window, &query_storage, &query_ids, &query, &query_language);
        }
    });

    let activate_storage = storage.clone();
    let activate_ids = Rc::clone(&ids);
    let activate_window = window.as_weak();
    window.on_activate_item(move |index| {
        let id = activate_ids.borrow().get(index as usize).cloned();
        if let Some(raw_id) = id.and_then(|id| id.strip_prefix("clipboard:").map(str::to_string)) {
            if let Ok(id) = raw_id.parse::<i64>() {
                if let Ok(Some(record)) = activate_storage.clipboard_by_id(id) {
                    let _ = set_clipboard_text(&record.content);
                    if let Some(window) = activate_window.upgrade() {
                        if window.get_popup_mode() {
                            let _ = window.hide();
                        }
                    }
                }
            }
        }
    });

    let convert_window = window.as_weak();
    let convert_storage = storage.clone();
    window.on_convert_to_window(move || {
        if let Some(window) = convert_window.upgrade() {
            let dark_mode = window.get_dark_mode();
            let language = window.get_language().to_string();
            window.hide().ok();
            open_clipboard_window(convert_storage.clone(), dark_mode, language);
        }
    });

    let pin_window = window.as_weak();
    let pin_storage = storage.clone();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            if window.get_popup_mode() {
                let dark_mode = window.get_dark_mode();
                let language = window.get_language().to_string();
                window.hide().ok();
                open_pinned_clipboard_window(pin_storage.clone(), dark_mode, language);
            } else {
                window.set_pinned(!window.get_pinned());
            }
        }
    });

    ClipboardWindowState { window, ids }
}

/// 刷新并重新显示已有的剪贴板窗口，确保托盘重复点击不会创建窗口副本。
pub(super) fn show_existing_clipboard_window(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    pin: bool,
) -> bool {
    CLIPBOARD_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let Some(state) = windows
            .iter()
            .rev()
            .find(|state| !state.window.get_popup_mode())
        else {
            return false;
        };

        if pin {
            state.window.set_pinned(true);
        }
        state.window.set_dark_mode(dark_mode);
        state.window.set_language(language.into());
        refresh_clipboard_window(&state.window, storage, &state.ids, "", language);
        show_window_in_foreground(&state.window.window());
        true
    })
}

/// 刷新并重新显示已有的托盘剪贴板弹窗，同时更新其位置。
pub(super) fn show_existing_clipboard_popup(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    position: TrayPosition,
) -> bool {
    CLIPBOARD_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let Some(state) = windows
            .iter()
            .rev()
            .find(|state| state.window.get_popup_mode())
        else {
            return false;
        };

        state.window.set_dark_mode(dark_mode);
        state.window.set_language(language.into());
        refresh_clipboard_window(&state.window, storage, &state.ids, "", language);
        set_clipboard_popup_position(&state.window, position);
        show_window_in_foreground(&state.window.window());
        true
    })
}

pub(super) fn set_clipboard_popup_position(window: &ClipboardWindow, position: TrayPosition) {
    // 窗口尺寸已在创建时显式设为 420x620 逻辑像素,这里按逻辑尺寸×scale 计算。
    // 注意不能用 window.size():隐藏窗口的尺寸缓存可能是旧值(见 build_clipboard_window)。
    let scale = window.window().scale_factor().max(1.0);
    let width = (420.0 * scale).round() as i32;
    #[cfg(not(target_os = "macos"))]
    let height = (620.0 * scale).round() as i32;

    #[cfg(target_os = "macos")]
    let x = position.x.saturating_sub(width / 2);
    #[cfg(not(target_os = "macos"))]
    let x = position.x.saturating_sub(width).saturating_add(32);

    #[cfg(target_os = "macos")]
    let y = position.y.saturating_add(8);
    #[cfg(not(target_os = "macos"))]
    let y = position.y.saturating_sub(height).saturating_sub(8);

    window
        .window()
        .set_position(slint::PhysicalPosition::new(x, y));
}
