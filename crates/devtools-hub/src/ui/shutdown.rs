//! 全应用统一收尾：先停止新任务，再保存持久状态、释放窗口与钩子，最后等待后台线程退出。

use super::{lock_search, search_window, HubApp, OcrView, Page};
use crate::{platform, shutdown, update};
use std::time::Instant;

impl HubApp {
    pub(super) fn prepare_exit(&mut self) {
        if self.shutdown_started {
            return;
        }
        self.shutdown_started = true;
        self.shared.request_exit(false);
        self.shared
            .quit_consumed
            .store(true, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + shutdown::CLEANUP_BUDGET;
        tracing::info!(
            "开始清理应用，系统会话退出：{}",
            self.shared.shutdown.system_session()
        );
        update::shutdown(&self.shared);
        if let Some(hotkeys) = &mut self.hotkeys {
            for action in crate::hotkey::HotkeyAction::ALL {
                hotkeys.suspend(action);
            }
        }
        self.hotkeys = None;
        self.tray = None;
        #[cfg(windows)]
        if let Some(worker) = platform::mouse::stop() {
            self.shared.shutdown.track(worker);
        }
        self.shutdown_screenshots();
        self.ocr = OcrView::default();
        if let Some(worker) = self
            .panel_queries
            .take()
            .and_then(|worker| worker.shutdown())
        {
            self.shared.shutdown.track(worker);
        }
        if let Some(handle) = self.search.take() {
            let mut search = lock_search(&handle);
            if self.search_open {
                if let Some(rect) = self.ctx.input(|input| {
                    input
                        .raw
                        .viewports
                        .get(&search_window::viewport())
                        .and_then(|view| view.inner_rect)
                }) {
                    if rect.width() > 100.0 && rect.height() > 100.0 {
                        search.remember_size([rect.width().round(), rect.height().round()]);
                    }
                }
                search.on_hidden();
            }
            search.shutdown();
        }
        self.search_open = false;
        if self.page == Page::Panel && !platform::panel::is_waking() {
            if let Some(rect) = self.ctx.input(|input| input.viewport().inner_rect) {
                if rect.width() > 100.0 && rect.height() > 100.0 {
                    self.settings.window_size = [rect.width().round(), rect.height().round()];
                }
            }
        }
        // 退出时直接写设置，不能用 persist_settings 再次启动搜索或注册快捷键。
        if let Err(error) = self.settings.save(&self.data_dir) {
            tracing::warn!("退出时保存设置失败：{error:#}");
        }
        self.textures.clear();
        self.app_icons.clear();
        self.panel.items.clear();
        self.panel.release_preview();
        self.code.clear();
        if let Some(liquid) = &self.liquid {
            liquid.release_targets();
        }
        let remaining = self.shared.shutdown.join_until(deadline);
        if !remaining.is_empty() {
            tracing::warn!("清理等待超时，未结束的任务：{remaining:?}");
        }
        if let Err(error) = self.store.checkpoint() {
            tracing::warn!("退出时合并剪贴板数据库日志失败：{error:#}");
        }
        self.shared.shutdown.finish();
        tracing::info!("应用收尾完成");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::shared::{AppEvent, Shared};
    use devtools_clipboard::Store;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    #[test]
    fn confirmed_exit_saves_settings_stops_workers_and_closes_pins_once() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        let shared = Shared::new(&settings);
        let mut app = HubApp::build(
            egui::Context::default(),
            shared.clone(),
            Store::open(dir.path()).unwrap(),
            dir.path().to_path_buf(),
            settings,
        );
        app.settings.poll_interval_ms = 750;
        let mut search = devtools_search::SearchModule::new(dir.path(), app.search_host.clone());
        search
            .service_mut()
            .handle(devtools_search::application::protocol::Command::Search {
                text: "退出前待保存的搜索".into(),
                literal: true,
                case_sensitive: false,
                scope: None,
            });
        app.search = Some(Arc::new(std::sync::Mutex::new(search)));
        app.search_open = true;
        app.pins.push(super::super::pin::Pin::new(
            9001,
            image::RgbaImage::new(8, 8),
            egui::vec2(8.0, 8.0),
            1.0,
            egui::Pos2::ZERO,
        ));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let worker_shared = shared.clone();
        shared.shutdown.track(std::thread::spawn(move || {
            worker_shared
                .shutdown
                .wait_requested(Duration::from_secs(30));
            worker_stopped.store(true, Ordering::Release);
        }));
        app.panel_queries = Some(
            super::super::panel::query::PanelQueries::spawn(
                dir.path().to_path_buf(),
                app.ctx.clone(),
            )
            .unwrap(),
        );
        shared.request_exit(true);
        app.prepare_exit();
        assert!(shared.shutdown.system_session());
        assert!(shared.shutdown.wait_finished(Duration::ZERO));
        assert!(stopped.load(Ordering::Acquire));
        assert!(app.pins.is_empty());
        assert!(app.panel_queries.is_none());
        assert!(app.search.is_none());
        let saved_search =
            devtools_search::settings::SearchSettingsStore::new(&dir.path().join("search")).load();
        assert_eq!(saved_search.settings.history[0].query, "退出前待保存的搜索");
        assert_eq!(Settings::load(dir.path()).unwrap().poll_interval_ms, 750);
        shared.drain_events();
        shared.post(AppEvent::Snip);
        assert!(shared.drain_events().is_empty(), "退出期间不再接受新任务");
        // 第二次 on_exit 不应重复写设置或重新创建服务。
        app.settings.poll_interval_ms = 900;
        app.prepare_exit();
        assert_eq!(Settings::load(dir.path()).unwrap().poll_interval_ms, 750);
    }
}
