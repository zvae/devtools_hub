//! 服务端到端测试：真实文件系统上扫描 → 索引 → 查询 → 取行（不涉及 UI）。
//!
//! 验证 §7 的主干链路：排除规则、字面查询、过滤器、路径分段、LoadRows 与快照落盘。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devtools_search::application::protocol::{Command, Event, RowData};
use devtools_search::application::service::{Lifecycle, Service};
use devtools_search::settings::{RootConfig, RootLocation, SearchSettings, VolumeId};
use devtools_search::{HostServices, SystemModalGuard};

#[derive(Default)]
struct FakeHost {
    opened: Mutex<Vec<PathBuf>>,
    revealed: Mutex<Vec<PathBuf>>,
}

impl HostServices for FakeHost {
    fn pick_folder(&self, _initial: Option<&Path>) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }
    fn begin_system_modal(&self) -> Option<Box<dyn SystemModalGuard>> {
        None
    }
    fn open(&self, path: &Path) -> io::Result<()> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.revealed.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn icon(&self, _path: &Path) -> Option<(u32, u32, Vec<u8>)> {
        None
    }
    fn set_search_space_behavior(&self, _searching: bool) {}
    fn request_repaint(&self) {}
    fn start_window_drag(&self) {}
}

/// 临时目录里放几个文件，包含一个应被排除的 node_modules。
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "hello").unwrap();
    std::fs::write(dir.path().join("beta.PDF"), "pdf").unwrap();
    std::fs::write(dir.path().join("gamma.md"), "# md").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/delta.txt"), "delta").unwrap();
    std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    std::fs::write(dir.path().join("node_modules/pkg/skip.js"), "x").unwrap();
    dir
}

fn root_for(path: &Path) -> RootLocation {
    RootLocation {
        volume_id: VolumeId::fallback_absolute(path),
        relative_path: String::new(),
        fallback_absolute_path: path.to_path_buf(),
    }
}

fn settings_for(root: &Path) -> SearchSettings {
    let mut settings = SearchSettings::default();
    settings.roots = vec![RootConfig {
        location: root_for(root),
        enabled: true,
    }];
    settings.exclude_patterns = vec!["node_modules".into()];
    settings.exclude_case_sensitive = false;
    settings.lazy_start = false;
    settings.persist_index = true;
    settings.permission_onboarding_completed = true;
    settings.history_enabled = false;
    settings.show_all_on_empty = false;
    settings.debounce_ms = 1;
    settings.ui.window.size = [1100.0, 680.0];
    settings
}

/// 轮询事件直到条件满足或超时。
fn wait_for(
    service: &mut Service,
    timeout: Duration,
    mut predicate: impl FnMut(&Event) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for event in service.poll().events {
            if predicate(&event) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// 等到所有根就绪（扫描完成）。
fn wait_ready(service: &mut Service) {
    use devtools_search_core::volumes::RootState;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let roots = service.roots();
        if !roots.is_empty() && roots.iter().all(|r| matches!(r.state, RootState::Ready)) {
            return;
        }
        let _ = service.poll();
        std::thread::sleep(Duration::from_millis(15));
    }
    panic!("索引没有在 30 秒内就绪：{:?}", service.roots());
}

fn search(service: &mut Service, text: &str) -> Vec<RowData> {
    service.handle(Command::Search {
        text: text.to_string(),
        literal: false,
        case_sensitive: false,
        scope: None,
    });
    let done = wait_for(service, Duration::from_secs(20), |event| {
        matches!(event, Event::SearchDone(_))
    });
    assert!(done, "查询 {text:?} 没有返回结果");
    assert!(
        service.last_error().is_none(),
        "查询 {text:?} 报错：{:?}",
        service.last_error()
    );
    let len = service.results_len() as u32;
    service.handle(Command::LoadRows {
        start: 0,
        count: 200,
    });
    let mut rows = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while rows.len() < len as usize && Instant::now() < deadline {
        for event in service.poll().events {
            if let Event::Rows { rows: batch, .. } = event {
                rows.extend(batch);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    rows.retain(|row| !row.missing);
    rows
}

fn names(rows: &[RowData]) -> Vec<String> {
    let mut names: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn scans_indexes_searches_and_loads_rows() {
    let fixture = fixture();
    let data = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let mut service = Service::new(data.path(), host.clone());
    assert_eq!(service.lifecycle(), Lifecycle::AwaitingConsent);

    service.handle(Command::ApplySettings {
        draft: Box::new(settings_for(fixture.path())),
    });
    wait_ready(&mut service);
    assert_eq!(service.lifecycle(), Lifecycle::Indexing);

    // 子串查询。
    let rows = search(&mut service, "alpha");
    assert_eq!(names(&rows), ["alpha.txt"]);
    // 路径按父链拼出来（完整路径可用于打开/显示；根路径已规范化为 /private/var/…）。
    let expected = std::fs::canonicalize(fixture.path())
        .unwrap()
        .join("alpha.txt");
    assert_eq!(rows[0].path, expected.to_string_lossy());

    // 大小写不敏感（beta.PDF 用小写 pdf 能搜到）。
    let rows = search(&mut service, "pdf");
    assert_eq!(names(&rows), ["beta.PDF"]);

    // 过滤器。
    let rows = search(&mut service, "ext:md");
    assert_eq!(names(&rows), ["gamma.md"]);

    // 路径分段。
    let rows = search(&mut service, "sub/delta");
    assert_eq!(names(&rows), ["delta.txt"]);

    // 排除规则：node_modules 下不进入索引。
    assert!(search(&mut service, "skip").is_empty());
    assert!(search(&mut service, "node_modules").is_empty());

    // 空查询 + show_all_on_empty=false → 没有结果。
    assert!(search(&mut service, "").is_empty());

    // 打开/显示走宿主。
    let path = fixture.path().join("alpha.txt");
    service.handle(Command::Open { path: path.clone() });
    service.handle(Command::Reveal { path: path.clone() });
    let _ = service.poll();
    assert_eq!(host.opened.lock().unwrap().as_slice(), [path.clone()]);
    assert_eq!(host.revealed.lock().unwrap().as_slice(), [path]);

    // 快照落盘：设置文件与 manifest 都存在。
    let search_dir = data.path().join("search");
    assert!(search_dir.join("settings.json").is_file());
    assert!(
        search_dir.join("snapshots/manifest.json").is_file(),
        "应保存索引快照"
    );

    service.shutdown();
    assert!(service.is_stopped());
}

#[test]
fn manual_rebuild_picks_up_new_files() {
    let fixture = fixture();
    let data = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let mut service = Service::new(data.path(), host);
    service.handle(Command::ApplySettings {
        draft: Box::new(settings_for(fixture.path())),
    });
    wait_ready(&mut service);
    assert!(search(&mut service, "epsilon").is_empty());

    std::fs::write(fixture.path().join("epsilon.log"), "new").unwrap();
    service.handle(Command::RebuildAll);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let rows = search(&mut service, "epsilon");
        if names(&rows) == ["epsilon.log"] {
            break;
        }
        assert!(Instant::now() < deadline, "重建索引后应能搜到新文件");
        std::thread::sleep(Duration::from_millis(50));
    }
    service.shutdown();
}

#[test]
fn not_indexed_folder_reports_notice() {
    let fixture = fixture();
    let data = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let mut service = Service::new(data.path(), host);
    service.handle(Command::ApplySettings {
        draft: Box::new(settings_for(fixture.path())),
    });
    wait_ready(&mut service);

    service.handle(Command::Search {
        text: "infolder:/definitely/not/here".into(),
        literal: false,
        case_sensitive: false,
        scope: None,
    });
    assert!(wait_for(&mut service, Duration::from_secs(10), |event| {
        matches!(event, Event::SearchDone(_))
    }));
    assert_eq!(service.results_len(), 0);
    assert!(
        service
            .last_notice()
            .is_some_and(|notice| notice.contains("不在索引范围内")),
        "应给出范围提示：{:?}",
        service.last_notice()
    );
    service.shutdown();
}

/// 实时监听：新建 / 删除文件后，不重建索引也能出现在结果里（§8）。
#[test]
fn watch_updates_index_without_rebuild() {
    let fixture = fixture();
    let data = tempfile::tempdir().unwrap();
    let host = Arc::new(FakeHost::default());
    let mut service = Service::new(data.path(), host);
    let mut settings = settings_for(fixture.path());
    settings.watch = true;
    settings.persist_index = false;
    service.handle(Command::ApplySettings {
        draft: Box::new(settings),
    });
    wait_ready(&mut service);

    // 让服务挂上监听（poll 里维护）。
    for _ in 0..20 {
        let _ = service.poll();
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(search(&mut service, "watched").is_empty());

    // 新建文件：应自动出现在结果里。
    let created = fixture.path().join("watched.txt");
    std::fs::write(&created, "hello").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let rows = search(&mut service, "watched");
        if names(&rows) == ["watched.txt"] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "实时监听没有把新文件加进索引：{:?}",
            names(&rows)
        );
        let _ = service.poll();
        std::thread::sleep(Duration::from_millis(100));
    }

    // 删除文件：应自动从结果里消失。
    std::fs::remove_file(&created).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let rows = search(&mut service, "watched");
        if rows.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "实时监听没有把删除的文件移出索引：{:?}",
            names(&rows)
        );
        let _ = service.poll();
        std::thread::sleep(Duration::from_millis(100));
    }
    service.shutdown();
}
