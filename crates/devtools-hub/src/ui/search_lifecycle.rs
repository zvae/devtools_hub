//! 搜索窗口与模块生命周期，独立于宿主剪贴板和截图流程。

use super::{lock_search, search_window, HubApp, Palette};
use crate::platform::{floating, panel as native_panel};
use crate::shared::SearchEntry;
use devtools_search::{SearchModule, SearchOptions};
use std::path::PathBuf;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::time::{Duration, Instant};

impl HubApp {
    /// 搜索页是独立窗口：已经打开时热键只负责前置 / 收起，不再切换共享窗口的页面。
    pub(super) fn open_search(&mut self, entry: SearchEntry) {
        self.sync_search_module();
        if !self.settings.search_enabled {
            self.show_toast("文件搜索未启用，可在设置的“文件搜索”中开启");
            return;
        }
        let Some(handle) = self.search.as_ref().cloned() else {
            return;
        };
        let options = match entry {
            SearchEntry::Hotkey => None,
            SearchEntry::Text(text) => Some(SearchOptions {
                query: text,
                selected_paths: Vec::new(),
                query_is_literal: true,
                case_sensitive: self.settings.search_enabled
                    && lock_search(&handle)
                        .service()
                        .settings()
                        .default_case_sensitive,
            }),
            SearchEntry::Paths(paths) => Some(SearchOptions {
                query: String::new(),
                selected_paths: paths.into_iter().map(PathBuf::from).collect(),
                query_is_literal: false,
                case_sensitive: false,
            }),
        };
        if self.search_open {
            let info = self
                .ctx
                .input(|i| i.raw.viewports.get(&search_window::viewport()).cloned());
            let Some(info) = info else {
                // 窗口已经随上一帧关闭，这里只清掉状态。
                self.close_search();
                return;
            };
            let minimized = info.minimized.unwrap_or(false);
            let focused = info.focused.unwrap_or(false);
            if !minimized && focused && options.is_none() {
                self.close_search();
                return;
            }
            // 打开了新入口（例如选中文件后再次唤起）：更新查询 / 范围并前置。
            if options.is_some() {
                lock_search(&handle).on_shown(options);
            }
            self.focus_search_window(minimized);
            return;
        }
        // 打开搜索时先收起剪贴板 / 快捷面板：搜索是普通窗口，面板是临时浮层。
        if native_panel::is_visible() {
            native_panel::hide(false);
        }
        // 已经被主动显示，就不要再执行启动时的隐藏。
        self.startup_hide_pending = false;
        self.search_open = true;
        self.search_close_requested.store(false, Ordering::Relaxed);
        self.search_focus_was.store(false, Ordering::Relaxed);
        *self
            .search_blur_grace_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.search_focus_pending = true;
        self.search_window_size = lock_search(&handle).window_size();
        // 出现在鼠标所在屏幕的中央；会话内保持这个位置（模式切换重建窗口也不跳）。
        self.search_window_pos = search_window::centered_position(
            self.search_window_size,
            crate::platform::screen::cursor().map(|(_, display)| display.logical),
        );
        {
            let mut search = lock_search(&handle);
            search.warm_up();
            search.on_shown(options);
        }
        // 面板隐藏时 eframe 只调用 logic；让面板以隐藏状态运行一帧界面，
        // 延迟视口（搜索窗口）才能被登记并创建（见 `panel::wake`）。
        native_panel::wake();
        native_panel::activate_search_window();
        self.apply_search_dock_visibility();
        self.ctx.request_repaint();
    }

    /// 关闭搜索窗口：保存尺寸、通知模块，并按需收起为创建子视口临时唤醒的面板。
    pub(super) fn close_search(&mut self) {
        let was_open = self.search_open;
        self.search_open = false;
        self.search_focus_pending = false;
        self.search_focus_was.store(false, Ordering::Relaxed);
        *self
            .search_blur_grace_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.search_close_requested.store(false, Ordering::Relaxed);
        if let Some(handle) = self.search.as_ref() {
            let mut search = lock_search(handle);
            if let Some(rect) = self.ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&search_window::viewport())
                    .and_then(|v| v.inner_rect)
            }) {
                if rect.width() > 100.0 && rect.height() > 100.0 {
                    search.remember_size([rect.width().round(), rect.height().round()]);
                }
            }
            if was_open {
                search.on_hidden();
            }
        }
        // 面板是隐藏的（可能正为创建子视口而临时唤醒）：把它真正收起来。
        // 面板本身可见时说明用户正在用剪贴板，不要跟着隐藏。
        if !native_panel::is_visible() {
            native_panel::finish_wake();
        }
        self.apply_search_dock_visibility();
    }

    /// 把搜索窗口前置；最小化时先恢复。
    pub(super) fn focus_search_window(&mut self, minimized: bool) {
        let id = search_window::viewport();
        if minimized {
            self.ctx
                .send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
        }
        self.ctx
            .send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
    }

    /// 登记搜索窗口的延迟视口。
    ///
    /// 用延迟（而不是立即）视口：立即视口必须在父窗口的界面回调里同步创建原生窗口，
    /// 而 eframe 在定时重绘隐藏窗口时不带事件循环上下文，创建会失败并 panic。
    /// 延迟视口的窗口由 eframe 在下一轮事件里创建，搜索窗口也能独立重绘。
    ///
    /// 停止登记后，egui 会在下一次界面结束时移除这个子视口，eframe 随之关闭原生窗口。
    pub(super) fn show_search_viewport(&mut self) {
        let Some(handle) = self.search.as_ref().cloned() else {
            self.close_search();
            return;
        };
        let id = search_window::viewport();
        let mode = lock_search(&handle).window_policy().mode;
        let builder = search_window::builder(mode, self.search_window_size, self.search_window_pos);
        // 窗口刚创建时系统激活可能还没完成：主动聚焦一次，之后不再抢焦点。
        if std::mem::take(&mut self.search_focus_pending) {
            self.ctx
                .send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        let close_requested = Arc::clone(&self.search_close_requested);
        let focus_was = Arc::clone(&self.search_focus_was);
        let blur_grace = Arc::clone(&self.search_blur_grace_until);
        let ui_handle = Arc::clone(&handle);
        self.ctx
            .show_viewport_deferred(id, builder, move |ui, _class| {
                // 搜索页画不透明底（§5.6）：普通窗口后面是桌面，玻璃/半透明会压低表格可读性。
                let background =
                    Palette::with_alpha(ui.ctx().theme() == egui::Theme::Dark, 255).background;
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(background))
                    .show(ui, |ui| {
                        let mut search = lock_search(&ui_handle);
                        let output = search.ui(ui);
                        let focused = ui.ctx().input(|i| i.viewport().focused).unwrap_or(false);
                        // 标题栏的关闭按钮：在子视口自己的输入里读取，避免事件被子视口重绘清掉。
                        let native_close = ui.ctx().input(|i| i.viewport().close_requested());
                        let lost_focus = focus_was.swap(focused, Ordering::Relaxed) && !focused;
                        if focused {
                            *blur_grace.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        }
                        let hide_on_blur = search.window_policy().mode.hide_on_blur();
                        drop(search);
                        // 切换窗口模式会临时改变应用的激活策略，可能带一次瞬时失焦：只忽略宽限期内的失焦。
                        let transient_blur = lost_focus && {
                            let mut grace = blur_grace.lock().unwrap_or_else(|p| p.into_inner());
                            let within = grace.is_some_and(|until| Instant::now() < until);
                            *grace = None;
                            within
                        };
                        // Esc / 关闭按钮 / 浮动模式失焦：交给宿主在下一帧统一收起。
                        if native_close
                            || output.close
                            || (lost_focus
                                && hide_on_blur
                                && !transient_blur
                                && !floating::dialog_open())
                        {
                            close_requested.store(true, Ordering::Relaxed);
                            // 延迟视口不会唤醒父窗口：主动让宿主跑一帧来处理关闭。
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                    });
            });
        // 用户拖动边框缩放后，把内容区尺寸记下来（关闭窗口时统一走 RememberSize）。
        if let Some(rect) = self
            .ctx
            .input(|i| i.raw.viewports.get(&id).and_then(|v| v.inner_rect))
        {
            if rect.width() > 100.0 && rect.height() > 100.0 {
                lock_search(&handle).remember_size([rect.width().round(), rect.height().round()]);
            }
        }
    }

    /// 普通窗口需要出现在 Dock（macOS）/ 任务栏（Windows）；浮动、固定模式保持常驻托盘。
    pub(super) fn apply_search_dock_visibility(&self) {
        let visible = self.search_open
            && self
                .search
                .as_ref()
                .is_some_and(|handle| lock_search(handle).window_policy().mode.in_taskbar());
        native_panel::set_search_dock_visible(visible);
    }

    /// 按开关创建 / 释放搜索模块（关闭时不再占线程，§4.4）。
    pub(super) fn sync_search_module(&mut self) {
        if self.settings.search_enabled {
            if self.search.is_none() && !self.search_stopping {
                let mut module = SearchModule::new(&self.data_dir, self.search_host.clone());
                module.warm_up();
                self.search = Some(Arc::new(Mutex::new(module)));
            }
        } else if let Some(handle) = &self.search {
            lock_search(handle).shutdown();
            self.search_stopping = true;
        }
        self.apply_hotkey(crate::hotkey::HotkeyAction::Search);
    }

    /// 拉取搜索模块的事件、响应策略变化、回收已停止的模块。
    pub(super) fn poll_search(&mut self) {
        // 视口回调按 Esc / 关闭按钮 / 浮动失焦时置位。
        if self.search_close_requested.swap(false, Ordering::Relaxed) {
            self.close_search();
            return;
        }
        // 原生标题栏的关闭按钮（普通窗口）。
        if self.search_open
            && self.ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&search_window::viewport())
                    .is_some_and(|v| v.close_requested())
            })
        {
            self.close_search();
            return;
        }
        let mut policy_changed = false;
        let mut stopped = false;
        if let Some(handle) = &self.search {
            let mut search = lock_search(handle);
            let poll = search.poll();
            policy_changed = poll.policy_changed;
            if let Some(delay) = poll.repaint_after {
                self.ctx
                    .request_repaint_after_for(delay, egui::ViewportId::ROOT);
            }
            stopped = search.is_stopped();
        }
        if policy_changed {
            // 普通 / 浮动 / 固定之间切换时同步 Dock 显示；窗口边框与层级由下一帧的 builder 更新。
            // 切换激活策略可能带一次瞬时失焦，浮动模式不要据此把窗口收起来。
            *self
                .search_blur_grace_until
                .lock()
                .unwrap_or_else(|p| p.into_inner()) =
                Some(Instant::now() + Duration::from_millis(600));
            self.apply_search_dock_visibility();
            if self.search_open {
                // 切换模式时窗口会重建 / 改层级，重新聚焦一次，避免切到浮动模式后停在“未聚焦”状态。
                self.ctx
                    .send_viewport_cmd_to(search_window::viewport(), egui::ViewportCommand::Focus);
            }
        }
        if self.search_stopping && stopped {
            self.search = None;
            self.search_stopping = false;
            if self.search_open {
                self.close_search();
            }
        }
        // 服务线程通过 ROOT 唤醒界面；延迟视口不会跟着父窗口重绘，这里转发一次。
        if self.search_open {
            self.ctx.request_repaint_of(search_window::viewport());
        }
    }
}
