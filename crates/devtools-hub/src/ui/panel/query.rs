//! 剪贴板查询：单工作线程、合并待处理请求、丢弃迟到结果。

use super::{Filter, HubApp, QUERY_LIMIT};
use anyhow::Result;
use devtools_clipboard::{ClipItem, ClipList, ItemQuery, Store};
use std::path::PathBuf;
use std::sync::{
    mpsc::{self, SyncSender},
    Arc, Mutex,
};
use std::time::Duration;

struct LoadedItems {
    items: Vec<ClipItem>,
    total: i64,
    lists: Vec<ClipList>,
}
struct Request {
    generation: u64,
    query: ItemQuery,
}
struct Reply {
    generation: u64,
    result: Result<LoadedItems, String>,
}

pub(crate) struct PanelQueries {
    wake: SyncSender<()>,
    pending: Arc<Mutex<Option<Request>>>,
    reply: Arc<Mutex<Option<Reply>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

fn load(store: &Store, query: &ItemQuery) -> Result<LoadedItems> {
    Ok(LoadedItems {
        items: store.items(query)?,
        total: store.count(query)?,
        lists: store.lists()?,
    })
}

impl PanelQueries {
    pub fn spawn(dir: PathBuf, ctx: egui::Context) -> Result<Self> {
        let (wake, rx) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(None::<Request>));
        let reply = Arc::new(Mutex::new(None));
        let worker_pending = pending.clone();
        let worker_reply = reply.clone();
        let worker = std::thread::Builder::new()
            .name("clipboard-query".into())
            .spawn(move || {
                let mut store = None;
                while rx.recv().is_ok() {
                    let Some(request) = worker_pending.lock().unwrap().take() else {
                        continue;
                    };
                    let result = (|| {
                        if store.is_none() {
                            store = Some(Store::open(&dir)?);
                        }
                        load(store.as_ref().unwrap(), &request.query)
                    })()
                    .map_err(|error: anyhow::Error| format!("{error:#}"));
                    *worker_reply.lock().unwrap() = Some(Reply {
                        generation: request.generation,
                        result,
                    });
                    ctx.request_repaint();
                }
            })?;
        Ok(Self {
            wake,
            pending,
            reply,
            worker: Some(worker),
        })
    }

    fn request(&self, request: Request) -> Result<(), String> {
        *self.pending.lock().unwrap() = Some(request);
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(())) => Err("剪贴板查询线程已停止".into()),
        }
    }

    /// 关闭请求通道，丢弃尚未开始的查询；调用方统一等待正在读取的数据库连接释放。
    pub fn shutdown(mut self) -> Option<std::thread::JoinHandle<()>> {
        self.pending.lock().unwrap().take();
        self.worker.take()
    }
}

impl HubApp {
    pub(in crate::ui) fn reload_items(&mut self) {
        // 测试构造器只验证绘制及操作；正式应用在 new 中接入后台查询。
        if self.panel_queries.is_some() {
            if let Some(changed) = self.panel.query_changed_at {
                let remaining = Duration::from_millis(120).saturating_sub(changed.elapsed());
                if !remaining.is_zero() {
                    self.ctx.request_repaint_after(remaining);
                    return;
                }
            }
        }
        self.panel.query_changed_at = None;
        self.panel.needs_reload = false;
        self.panel.query_generation = self.panel.query_generation.wrapping_add(1);
        let (kind, list_id) = self.panel.filter.to_query();
        let query = ItemQuery {
            text: self.panel.query.clone(),
            kind,
            list_id,
            limit: QUERY_LIMIT,
        };
        if let Some(worker) = &self.panel_queries {
            self.panel.query_pending = true;
            if let Err(error) = worker.request(Request {
                generation: self.panel.query_generation,
                query,
            }) {
                self.panel.error = Some(error);
                self.panel.query_pending = false;
            }
        } else {
            #[cfg(test)]
            self.apply_panel_items(load(&self.store, &query).map_err(|e| format!("{e:#}")));
        }
    }

    pub(in crate::ui) fn poll_panel_query(&mut self) {
        let reply = self
            .panel_queries
            .as_ref()
            .and_then(|worker| worker.reply.lock().unwrap().take());
        if let Some(reply) = reply {
            if !self.panel.needs_reload && reply.generation == self.panel.query_generation {
                self.apply_panel_items(reply.result);
            }
        }
    }

    fn apply_panel_items(&mut self, loaded: Result<LoadedItems, String>) {
        self.panel.query_pending = false;
        let previous = self.panel.selected_item().map(|item| item.id);
        match loaded {
            Ok(LoadedItems {
                items,
                total,
                lists,
            }) => {
                self.panel.items = items;
                self.panel.total = total;
                self.panel.lists = lists;
                self.panel.error = None;
            }
            Err(error) => {
                self.panel.error = Some(format!("读取历史失败：{error}"));
                return;
            }
        }
        if let Filter::List(id) = self.panel.filter {
            if !self.panel.lists.iter().any(|list| list.id == id) {
                self.panel.set_filter(Filter::All);
                self.ctx.request_repaint();
            }
        }
        if !self.panel.scroll_to_selected {
            if let Some(index) =
                previous.and_then(|id| self.panel.items.iter().position(|item| item.id == id))
            {
                self.panel.selected = index;
            }
        }
        self.panel.selected = self
            .panel
            .selected
            .min(self.panel.items.len().saturating_sub(1));
        self.panel.preview = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::shared::Shared;
    use std::time::Instant;

    #[test]
    fn background_query_returns_latest_requested_generation() {
        let dir = tempfile::tempdir().unwrap();
        let worker =
            PanelQueries::spawn(dir.path().to_path_buf(), egui::Context::default()).unwrap();
        for generation in 1..=20 {
            worker
                .request(Request {
                    generation,
                    query: ItemQuery::default(),
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(reply) = worker.reply.lock().unwrap().take() {
                if reply.generation == 20 {
                    assert_eq!(reply.result.unwrap().total, 0);
                    break;
                }
            }
            assert!(Instant::now() < deadline, "后台查询未返回最新请求");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn panel_discards_stale_reply_and_applies_current_reply() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        let mut app = HubApp::build(
            egui::Context::default(),
            Shared::new(&settings),
            Store::open(dir.path()).unwrap(),
            dir.path().to_path_buf(),
            settings,
        );
        let (wake, _rx) = mpsc::sync_channel(1);
        app.panel_queries = Some(PanelQueries {
            wake,
            pending: Arc::new(Mutex::new(None)),
            reply: Arc::new(Mutex::new(None)),
            worker: None,
        });
        app.panel.query_generation = 2;
        app.panel.total = 7;
        let set_reply = |app: &HubApp, generation| {
            *app.panel_queries.as_ref().unwrap().reply.lock().unwrap() = Some(Reply {
                generation,
                result: Ok(LoadedItems {
                    items: vec![],
                    lists: vec![],
                    total: 99,
                }),
            });
        };
        set_reply(&app, 1);
        app.poll_panel_query();
        assert_eq!(app.panel.total, 7);
        set_reply(&app, 2);
        app.poll_panel_query();
        assert_eq!(app.panel.total, 99);

        // 条件已变化但仍在防抖时，上一请求的结果也必须丢弃。
        app.panel.query = "new query".into();
        app.panel.invalidate_query();
        set_reply(&app, 2);
        app.poll_panel_query();
        assert_eq!(app.panel.total, 0);
        assert!(app.panel.selected_item().is_none());
    }
}
