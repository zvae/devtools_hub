//! 离屏渲染测试：用示例数据渲染面板、设置页、鼠标中键快捷面板、JSON 编辑器和提取文字，
//! 输出 PNG 供人工检查，并验证键盘操作。
//!
//! 需要 GPU（wgpu/Metal），默认忽略。手动运行：
//! `cargo test -p devtools-hub snapshot -- --ignored`
//! 输出目录：`target/ui-snapshots/`（可用环境变量 `UI_SNAPSHOT_DIR` 覆盖）。

use std::io::Cursor;
use std::path::PathBuf;

use devtools_clipboard::store::now_ms;
use devtools_clipboard::{
    thumbnail, uti, Capture, ContentKind, Representation, Snapshot, SourceApp, Store,
    FAVORITES_LIST_ID,
};
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use image::{ImageFormat, Rgba, RgbaImage};

use super::native_menu::Entry;
use super::panel::{tab_menu, Action};
use super::{style, HubApp, Page, PasteMode};
use crate::platform::Backdrop;
use crate::selection::Selection;
use crate::settings::{Settings, Tab, TabSetting};
use crate::shared::{AppEvent, Shared};
use crate::tools::QuickAction;
use crate::update::{Asset, Install, Release, Status};

fn source(bundle_id: &str, name: &str) -> SourceApp {
    SourceApp {
        bundle_id: Some(bundle_id.into()),
        name: Some(name.into()),
    }
}

fn record(store: &mut Store, snapshot: Snapshot, source: SourceApp, minutes_ago: i64) -> i64 {
    let analysis = uti::analyze(&snapshot);
    let thumb = analysis
        .image_uti
        .and_then(|ty| snapshot.first(ty))
        .map(|bytes| thumbnail::make(bytes).unwrap());
    let capture = Capture {
        snapshot,
        source,
        captured_at_ms: now_ms() - minutes_ago * 60_000,
    };
    store
        .record(&capture, &analysis, thumb.as_ref())
        .unwrap()
        .id
}

fn sample_png() -> Vec<u8> {
    let image = RgbaImage::from_fn(1280, 720, |x, y| {
        let r = (x * 255 / 1280) as u8;
        let g = (y * 255 / 720) as u8;
        Rgba([r, g, 200, 255])
    });
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    png
}

fn seed(store: &mut Store) {
    let png = sample_png();
    record(
        store,
        Snapshot {
            items: vec![
                vec![Representation::new(
                    uti::FILE_URL,
                    b"file:///Users/me/Documents/%E6%8A%A5%E5%91%8A.pdf".to_vec(),
                )],
                vec![Representation::new(
                    uti::FILE_URL,
                    b"file:///Users/me/Downloads/archive.zip".to_vec(),
                )],
            ],
        },
        source("com.apple.finder", "访达"),
        300,
    );
    record(
        store,
        Snapshot {
            items: vec![vec![
                Representation::new(
                    uti::PLAIN_TEXT,
                    "会议纪要：周五前完成剪贴板历史功能的验收".as_bytes(),
                ),
                Representation::new(uti::HTML, "<b>会议纪要</b>".as_bytes()),
            ]],
        },
        source("com.apple.Notes", "备忘录"),
        90,
    );
    record(
        store,
        Snapshot {
            items: vec![vec![Representation::new(uti::PNG, png)]],
        },
        source("com.apple.screencaptureui", "截屏"),
        30,
    );
    record(
        store,
        Snapshot::from_text("https://github.com/emilk/egui"),
        source("com.apple.Safari", "Safari 浏览器"),
        12,
    );
    // VS Code 复制代码时同时写入带语法高亮的 HTML（深色主题的配色）。
    let code = record(
        store,
        Snapshot {
            items: vec![vec![
                Representation::new(uti::PLAIN_TEXT, CODE_TEXT.as_bytes()),
                Representation::new(uti::HTML, CODE_HTML.as_bytes()),
            ]],
        },
        source("com.microsoft.VSCode", "Visual Studio Code"),
        3,
    );
    record(
        store,
        Snapshot::from_text("{\"name\": \"devtools-hub\", \"version\": \"0.1.0\"}"),
        source("com.postmanlabs.mac", "Postman"),
        1,
    );
    // 同一内容复制两次：只保留一条，复制次数为 2。
    record(
        store,
        Snapshot::from_text("{\"name\": \"devtools-hub\", \"version\": \"0.1.0\"}"),
        source("com.apple.Terminal", "终端"),
        0,
    );
    store.set_in_list(code, FAVORITES_LIST_ID, true).unwrap();
}

const CODE_TEXT: &str =
    "fn main() {\n    let history = Store::open(dir)?;\n    println!(\"{} items\", history.len());\n}\n";
const CODE_HTML: &str = concat!(
    "<meta charset='utf-8'><div style=\"color: #cccccc;background-color: #1f1f1f;",
    "font-family: Menlo, Monaco, 'Courier New', monospace;font-size: 12px;white-space: pre;\">",
    "<div><span style=\"color: #569cd6;\">fn</span><span style=\"color: #cccccc;\"> </span>",
    "<span style=\"color: #dcdcaa;\">main</span><span style=\"color: #cccccc;\">() {</span></div>",
    "<div><span style=\"color: #cccccc;\">    </span><span style=\"color: #569cd6;\">let</span>",
    "<span style=\"color: #cccccc;\"> </span><span style=\"color: #9cdcfe;\">history</span>",
    "<span style=\"color: #cccccc;\"> = </span><span style=\"color: #4ec9b0;\">Store</span>",
    "<span style=\"color: #cccccc;\">::</span><span style=\"color: #dcdcaa;\">open</span>",
    "<span style=\"color: #cccccc;\">(</span><span style=\"color: #9cdcfe;\">dir</span>",
    "<span style=\"color: #cccccc;\">)?;</span></div>",
    "<div><span style=\"color: #cccccc;\">    </span><span style=\"color: #dcdcaa;\">println!</span>",
    "<span style=\"color: #cccccc;\">(</span><span style=\"color: #ce9178;\">&quot;{} items&quot;</span>",
    "<span style=\"color: #cccccc;\">, </span><span style=\"color: #9cdcfe;\">history</span>",
    "<span style=\"color: #cccccc;\">.</span><span style=\"color: #dcdcaa;\">len</span>",
    "<span style=\"color: #cccccc;\">());</span></div>",
    "<div><span style=\"color: #cccccc;\">}</span></div></div>",
);

fn output_dir() -> PathBuf {
    let dir = std::env::var_os("UI_SNAPSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/ui-snapshots"
            ))
        });
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(harness: &mut Harness<'_, HubApp>, name: &str) {
    let image = harness.render().expect("离屏渲染失败");
    let path = output_dir().join(format!("{name}.png"));
    image.save(&path).unwrap();
    eprintln!("已保存 {}", path.display());
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_panel_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    seed(&mut store);
    let settings = Settings::default();
    let shared = Shared::new(&settings);
    let app = HubApp::build(
        egui::Context::default(),
        shared,
        store,
        dir.path().to_path_buf(),
        settings,
    );

    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(380.0, 640.0))
            .with_pixels_per_point(2.0)
            .wgpu()
            .build_ui_state(
                |ui, app: &mut HubApp| {
                    app.handle_events();
                    app.draw(ui);
                },
                HubApp::build(
                    egui::Context::default(),
                    app.shared.clone(),
                    Store::open(dir.path()).unwrap(),
                    dir.path().to_path_buf(),
                    Settings::default(),
                ),
            );
        let ctx = harness.ctx.clone();
        style::install(&ctx);
        ctx.set_theme(theme);
        harness.state_mut().ctx = ctx;
        harness.state_mut().panel.reset_for_show();
        harness.run_steps(4);

        let panel = &harness.state().panel;
        assert_eq!(panel.total, 6);
        assert_eq!(panel.items.len(), 6, "重复复制的 JSON 应被去重");
        assert_eq!(panel.items[0].copy_count, 2);
        save(&mut harness, &format!("panel-{suffix}"));

        // ↓↓↓ 选中第四条（截图），预览区应显示图片。
        for _ in 0..3 {
            harness.key_press(Key::ArrowDown);
            harness.run_steps(1);
        }
        harness.run_steps(2);
        assert_eq!(harness.state().panel.selected, 3);
        assert_eq!(
            harness.state().panel.items[3].kind,
            devtools_clipboard::ContentKind::Image
        );
        save(&mut harness, &format!("panel-image-{suffix}"));

        // 悬停底栏的键盘按钮，显示快捷键说明。
        harness
            .ctx
            .all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
        harness.hover_at(egui::pos2(356.0, 621.0));
        harness.run_steps(4);
        save(&mut harness, &format!("shortcuts-{suffix}"));
        harness.hover_at(egui::pos2(200.0, 10.0));
        harness.run_steps(2);

        // 空格打开大预览，esc 关闭。
        harness.key_press(Key::Space);
        harness.run_steps(3);
        assert!(harness.state().panel.preview_open);
        save(&mut harness, &format!("preview-{suffix}"));
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().panel.preview_open);

        // 输入关键字过滤。
        harness.state_mut().panel.query = "会议".into();
        harness.state_mut().panel.needs_reload = true;
        harness.run_steps(3);
        assert_eq!(harness.state().panel.items.len(), 1);
        save(&mut harness, &format!("panel-search-{suffix}"));

        // Esc 清空搜索，⇥ 切换到“文本”分类。
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        harness.key_press(Key::Tab);
        harness.run_steps(3);
        assert!(harness
            .state()
            .panel
            .items
            .iter()
            .all(|item| item.kind.is_textual()));

        // ⌘, 打开设置页。
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
        harness.run_steps(3);
        assert_eq!(harness.state().page, Page::Settings);
        save(&mut harness, &format!("settings-{suffix}"));
        // 拉高窗口，检查整页设置的布局。
        harness.set_size(egui::vec2(380.0, 1900.0));
        harness.run_steps(3);
        save(&mut harness, &format!("settings-full-{suffix}"));
        // 窗口背景是液态玻璃时多出“外观”分组（背景透明度滑块），底色变成半透明。
        harness.state_mut().backdrop = Backdrop::Glass;
        harness.run_steps(3);
        save(&mut harness, &format!("settings-glass-{suffix}"));
    }
}

/// uTools 截图中选中的那段 JSON（单行），外加一个嵌套数组。
const SELECTED_JSON: &str = r#"{"menuId": 701011,"menuName": "预算和投入","menuNameEn": "","parentId": 7010,"icon": "","routePath": "bugetInvest","sort": 2.0,"visible": true,"subList": [{"menuId": 70101101, "menuName": "预算编制", "todoCount": 3}],"todoCount": 0}"#;

fn harness_for(
    dir: &std::path::Path,
    theme: egui::Theme,
    size: egui::Vec2,
    setup: impl FnOnce(&mut HubApp),
) -> Harness<'static, HubApp> {
    let settings = Settings::default();
    let app = HubApp::build(
        egui::Context::default(),
        Shared::new(&settings),
        Store::open(dir).unwrap(),
        dir.to_path_buf(),
        settings,
    );
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut HubApp| {
                app.handle_events();
                app.draw(ui);
            },
            app,
        );
    let ctx = harness.ctx.clone();
    style::install(&ctx);
    ctx.set_theme(theme);
    ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    harness.state_mut().ctx = ctx;
    // 字体在上面才装进 harness 的 Context，页面要在这之后再切换。
    setup(harness.state_mut());
    harness.run_steps(3);
    harness
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_code() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 640.0), |app| {
            app.panel.reset_for_show();
        });
        // 第二条是从 VS Code 复制的代码：卡片和大预览都按高亮显示。
        harness.key_press(Key::ArrowDown);
        harness.run_steps(2);
        let item = &harness.state().panel.items[1];
        assert_eq!(item.kind, ContentKind::RichText);
        harness.key_press(Key::Space);
        harness.run_steps(4);
        assert!(harness.state().panel.preview_open);
        save(&mut harness, &format!("preview-code-{suffix}"));
    }
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_glass_header() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 520.0), |app| {
            app.panel.reset_for_show();
            // 不显示授权提示，顶部栏只有搜索框和分类标签。
            app.accessibility_ok = true;
        });
        harness.run_steps(3);
        // 往下滚：卡片滚到顶部栏下面，透过玻璃模糊地显示出来。
        harness.hover_at(egui::pos2(190.0, 300.0));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -120.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(10);
        save(&mut harness, &format!("glass-header-{suffix}"));
    }
}

/// 条目与分类标签的右键菜单是原生菜单，测试里不弹出，只检查菜单内容和选中后的操作。
#[test]
fn context_menu_entries() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    let settings = Settings::default();
    let mut app = HubApp::build(
        egui::Context::default(),
        Shared::new(&settings),
        Store::open(dir.path()).unwrap(),
        dir.path().to_path_buf(),
        settings,
    );
    app.panel.reset_for_show();
    app.reload_items();
    let files = app
        .panel
        .items
        .iter()
        .find(|item| item.kind == ContentKind::Files)
        .cloned()
        .unwrap();

    // 只有文字，与 PasteNow 一致；文件多出“在访达中显示”。
    let entries = app.item_menu(&files);
    let reveal = format!("在{}中显示", crate::platform::FILE_MANAGER);
    assert_eq!(
        labels(&entries),
        [
            "粘贴",
            "以纯文本粘贴",
            "仅复制",
            "—",
            "添加到列表",
            reveal.as_str(),
            "—",
            "删除",
            "—",
            "预览"
        ]
    );
    let Some(Entry::Submenu { entries: lists, .. }) = entries
        .iter()
        .find(|entry| entry.label() == Some("添加到列表"))
    else {
        panic!("应有“添加到列表”子菜单");
    };
    assert_eq!(labels(lists), ["收藏", "—", "新建列表…"]);
    assert!(matches!(lists[0], Entry::Check { checked: false, .. }));

    // 选中“收藏”：条目加入收藏，菜单里随之打勾。
    let favorite = take(entries, &["添加到列表", "收藏"]);
    assert_eq!(
        favorite,
        Action::SetInList {
            item: files.id,
            list: FAVORITES_LIST_ID,
            member: true,
        }
    );
    app.apply_actions(vec![favorite]);
    app.reload_items();
    let files = app
        .panel
        .items
        .iter()
        .find(|item| item.id == files.id)
        .unwrap();
    assert!(files.in_list(FAVORITES_LIST_ID));
    let entries = app.item_menu(files);
    assert_eq!(
        take(entries, &["粘贴"]),
        Action::Paste(files.id, PasteMode::Original)
    );

    // 分类标签：“全部”只能编辑；其它标签可以隐藏，隐藏后不再显示。
    assert_eq!(labels(&tab_menu(None)), ["编辑分类标签…"]);
    let entries = tab_menu(Some(Tab::Link));
    assert_eq!(labels(&entries), ["隐藏此标签", "编辑分类标签…"]);
    app.apply_actions(vec![take(entries, &["隐藏此标签"])]);
    assert!(app.settings.tabs.contains(&TabSetting {
        tab: Tab::Link,
        visible: false,
    }));
}

/// 菜单项的文字，分隔线写成“—”。
fn labels<A>(entries: &[Entry<A>]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.label().unwrap_or("—"))
        .collect()
}

/// 按文字路径（子菜单名、菜单项名）取出菜单项对应的操作。
fn take(entries: Vec<Entry<Action>>, path: &[&str]) -> Action {
    let (first, rest) = path.split_first().unwrap();
    let entry = entries
        .into_iter()
        .find(|entry| entry.label() == Some(first))
        .unwrap_or_else(|| panic!("找不到菜单项“{first}”"));
    match entry {
        Entry::Item { action, .. } | Entry::Check { action, .. } if rest.is_empty() => action,
        Entry::Submenu { entries, .. } => take(entries, rest),
        _ => panic!("菜单项“{first}”不能选中"),
    }
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_new_list_for_item() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 640.0), |app| {
            app.panel.reset_for_show();
        });
        // 右键最后一条（文件）：选中它。
        click_at(
            &mut harness,
            egui::pos2(190.0, MENU_CARD_Y),
            egui::PointerButton::Secondary,
        );
        assert_eq!(harness.state().panel.selected, 5);
        let id = harness.state().panel.items[5].id;

        // 菜单里的“新建列表…”：打开右上角的新建列表弹窗，输入框自动获得焦点。
        // 空格、回车不触发面板的预览与粘贴；回车新建列表并把这一条加进去。
        harness
            .state_mut()
            .apply_actions(vec![Action::NewList { add_item: Some(id) }]);
        harness.run_steps(4);
        assert!(egui::Popup::is_any_open(&harness.ctx));
        harness.event(egui::Event::Text("工作 笔记".into()));
        harness.run_steps(2);
        assert!(!harness.state().panel.preview_open);
        save(&mut harness, &format!("new-list-for-item-{suffix}"));
        harness.key_press(Key::Enter);
        harness.run_steps(4);
        assert!(!egui::Popup::is_any_open(&harness.ctx), "新建后弹窗应关闭");
        let panel = &harness.state().panel;
        let list = panel
            .lists
            .iter()
            .find(|l| l.name == "工作 笔记")
            .expect("应新建列表");
        let item = panel.items.iter().find(|item| item.id == id).unwrap();
        assert!(item.in_list(list.id));
        // 下一个主题从头开始。
        harness.state().store.delete_list(list.id).unwrap();
    }
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_tabs() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 640.0), |app| {
            app.panel.reset_for_show();
        });
        // 右上角“+”：新建列表弹窗，esc 只关闭弹窗，不收起面板。
        click_at(&mut harness, PLUS_BUTTON, egui::PointerButton::Primary);
        assert!(egui::Popup::is_any_open(&harness.ctx));
        save(&mut harness, &format!("new-list-popup-{suffix}"));
        harness.key_press(Key::Escape);
        harness.run_steps(3);
        assert!(!egui::Popup::is_any_open(&harness.ctx));
        assert_eq!(harness.state().page, Page::Panel);

        // 右键菜单里的“隐藏此标签”（隐藏“链接”）。
        harness
            .state_mut()
            .apply_actions(vec![Action::HideTab(Tab::Link)]);
        harness.run_steps(4);
        let tabs = harness.state().settings.tabs.clone();
        assert!(tabs.contains(&TabSetting {
            tab: Tab::Link,
            visible: false,
        }));
        save(&mut harness, &format!("tabs-hidden-{suffix}"));

        // 右键菜单里的“编辑分类标签…”：打开设置页并滚动到“分类标签”分组。
        harness.state_mut().apply_actions(vec![Action::EditTabs]);
        harness.run_steps(12);
        assert_eq!(harness.state().page, Page::Settings);
        save(&mut harness, &format!("settings-tabs-{suffix}"));

        // “收藏”上移一位，重新打开“链接”，返回面板后标签按新顺序显示。
        click_at(&mut harness, FAVORITES_UP, egui::PointerButton::Primary);
        click_at(&mut harness, LINK_TOGGLE, egui::PointerButton::Primary);
        let tabs: Vec<Tab> = harness
            .state()
            .settings
            .ordered_tabs(&[FAVORITES_LIST_ID])
            .into_iter()
            .filter(|setting| setting.visible)
            .map(|setting| setting.tab)
            .collect();
        assert_eq!(
            tabs,
            [
                Tab::Text,
                Tab::Link,
                Tab::Image,
                Tab::List(FAVORITES_LIST_ID),
                Tab::Files
            ]
        );
        harness.key_press(Key::Escape);
        harness.run_steps(4);
        assert_eq!(harness.state().page, Page::Panel);
        save(&mut harness, &format!("tabs-reordered-{suffix}"));

        // 恢复默认，下一个主题从头开始。
        harness.state_mut().settings.tabs.clear();
    }
}

/// 移动鼠标到 `pos` 并按下、松开 `button`。
fn click_at(harness: &mut Harness<'_, HubApp>, pos: egui::Pos2, button: egui::PointerButton) {
    harness.hover_at(pos);
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run_steps(4);
}

/// 在 `from` 按下，拖到 `to` 松开。
fn drag_between(harness: &mut Harness<'_, HubApp>, from: egui::Pos2, to: egui::Pos2) {
    harness.hover_at(from);
    harness.run_steps(1);
    harness.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(2);
    harness.hover_at(to);
    harness.run_steps(2);
    harness.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(3);
}

/// 380×640 的面板里右上角“+”的位置。
const PLUS_BUTTON: egui::Pos2 = egui::pos2(350.0, 68.0);
/// 设置页滚动到“分类标签”分组后，“收藏”一行的上移按钮与“链接”一行的开关。
const FAVORITES_UP: egui::Pos2 = egui::pos2(264.0, 295.0);
const LINK_TOGGLE: egui::Pos2 = egui::pos2(334.0, 178.0);

/// 面板里最后一张卡片（第 6 条）的纵坐标。
const MENU_CARD_Y: f32 = 580.0;

/// 快捷面板的窗口大小。harness 外层的 CentralPanel 四周各有 8 点边距，一并加上。
fn quick_size(app: &HubApp) -> egui::Vec2 {
    let [width, height] = app.quick.window_size();
    egui::vec2(width + 16.0, height + 16.0)
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_quick_panel_and_json_editor() {
    let dir = tempfile::tempdir().unwrap();
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        // 没有选中内容：列出常用功能。
        let mut probe = None;
        let mut harness = harness_for(dir.path(), theme, egui::vec2(300.0, 200.0), |app| {
            app.quick.open(None, false);
            app.page = Page::QuickActions;
            probe = Some(quick_size(app));
        });
        harness.set_size(probe.unwrap());
        harness.run_steps(2);
        save(&mut harness, &format!("quick-empty-{suffix}"));

        // 选中的文本不是 JSON：提示没有匹配的功能。
        let mut harness = harness_for(dir.path(), theme, egui::vec2(300.0, 200.0), |app| {
            app.quick
                .open(Some(Selection::Text("今天的会议改到下午三点".into())), false);
            app.page = Page::QuickActions;
            probe = Some(quick_size(app));
        });
        assert_eq!(
            harness.state().quick.actions,
            [QuickAction::ClipboardHistory]
        );
        harness.set_size(probe.unwrap());
        harness.run_steps(2);
        save(&mut harness, &format!("quick-unmatched-{suffix}"));

        // 选中 JSON：第一项是 JSON 编辑器，回车打开并自动格式化。
        let mut harness = harness_for(dir.path(), theme, egui::vec2(300.0, 200.0), |app| {
            app.quick.open(Some(Selection::Text(SELECTED_JSON.into())), false);
            app.page = Page::QuickActions;
            probe = Some(quick_size(app));
        });
        assert_eq!(
            harness.state().quick.actions,
            [QuickAction::JsonEditor, QuickAction::ClipboardHistory]
        );
        harness.set_size(probe.unwrap());
        harness.run_steps(2);
        save(&mut harness, &format!("quick-json-{suffix}"));
        harness.key_press(Key::ArrowDown);
        harness.run_steps(1);
        assert_eq!(harness.state().quick.selected, 1);
        harness.key_press(Key::ArrowUp);
        harness.run_steps(1);
        harness.key_press(Key::Enter);
        harness.run_steps(1);
        assert_eq!(harness.state().page, Page::JsonEditor);
        assert!(harness
            .state()
            .json
            .text
            .starts_with("{\n    \"menuId\": 701011,\n    \"menuName\": \"预算和投入\","));
        assert!(
            harness.state().json.text.contains("\"sort\": 2.0,"),
            "数字保持原样"
        );
        harness.set_size(egui::vec2(760.0, 560.0));
        harness.run_steps(4);
        save(&mut harness, &format!("json-editor-{suffix}"));

        // ⌥C 压缩并复制。
        harness.key_press_modifiers(Modifiers::ALT, Key::C);
        harness.run_steps(2);
        assert_eq!(
            harness.state().toast.as_ref().map(|t| t.text.as_str()),
            Some("已复制压缩后的 JSON")
        );
        assert!(!harness.state().json.text.contains('ç'), "⌥C 不应输入字符");

        // 路径过滤。
        harness.state_mut().json.filter = ".subList[].menuName".into();
        harness.run_steps(4);
        save(&mut harness, &format!("json-filter-{suffix}"));

        // 格式错误：底栏显示行列号，出错行标红。
        harness.state_mut().json.filter.clear();
        harness.state_mut().json.text = "{\n    \"a\": 1\n    \"b\": [1, 2,]\n}".into();
        harness.run_steps(4);
        save(&mut harness, &format!("json-error-{suffix}"));

        // esc 关闭编辑器。
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert_eq!(harness.state().page, Page::Panel);
    }
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_ocr() {
    use crate::snip::ocr::Line;

    // 1200×700 像素的“截图”：白底上 8 行深色的“文字”，第 3 行右边还有一段（表格的第二列）。
    let mut image = RgbaImage::from_pixel(1200, 700, Rgba([255, 255, 255, 255]));
    let mut lines = Vec::new();
    for row in 0..8u32 {
        let top = 40 + row * 80;
        let width = 400 + (row % 3) * 200;
        for y in top..top + 30 {
            for x in 40..40 + width {
                image.put_pixel(x, y, Rgba([50, 50, 56, 255]));
            }
        }
        lines.push(Line::new(
            format!("第 {} 行识别出的文字 Line {}", row + 1, row + 1),
            egui::Rect::from_min_size(egui::pos2(40.0, top as f32), egui::vec2(width as f32, 30.0)),
        ));
    }
    lines.push(Line::new(
        "第二列",
        egui::Rect::from_min_size(egui::pos2(1000.0, 200.0), egui::vec2(120.0, 30.0)),
    ));
    let dir = tempfile::tempdir().unwrap();
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        // 正在识别。
        let mut harness = harness_for(dir.path(), theme, egui::vec2(860.0, 560.0), |app| {
            app.pasteboard_name = Some(format!("devtools-hub-test-ocr-{}", std::process::id()));
            app.ocr.set_result(image.clone(), None);
            app.page = Page::Ocr;
        });
        harness.run_steps(3);
        save(&mut harness, &format!("ocr-running-{suffix}"));

        // 识别完成：左边的图片上不画框，右边是合并好的文字（同一行离得远的两段用制表符隔开）。
        harness
            .state_mut()
            .ocr
            .set_result(image.clone(), Some(lines.clone()));
        harness.run_steps(3);
        let text = harness.state().ocr.text().unwrap().to_owned();
        assert!(text.starts_with("第 1 行识别出的文字 Line 1\n"));
        assert!(text.contains("第 3 行识别出的文字 Line 3\t第二列\n"));
        save(&mut harness, &format!("ocr-done-{suffix}"));

        // 单击图片上的一行：选中整行（在右边定位）。
        click_at(
            &mut harness,
            egui::pos2(60.0, 78.0),
            egui::PointerButton::Primary,
        );
        let line = harness.state().ocr.highlight().expect("单击应当选中整行");
        assert_eq!(&text[line], "第 1 行识别出的文字 Line 1");

        // 用鼠标在图片上从第 1 行拖到第 4 行：跨行选中，右边标亮（可以细到字）。
        // 窗口 860×560 时图片在 (16, 60) 起、按 0.3375 缩放：第 1 行中线 y≈79，第 4 行 y≈160。
        drag_between(
            &mut harness,
            egui::pos2(60.0, 78.0),
            egui::pos2(60.0, 160.0),
        );
        let highlight = harness.state().ocr.highlight().expect("拖动应当选中文字");
        assert!(text[highlight].contains('\n'), "应当跨行选中");
        save(&mut harness, &format!("ocr-dragged-{suffix}"));

        // 在图片上选中第 4 行：右边对应的文字标亮。
        harness.state_mut().ocr.select_line(3);
        harness.run_steps(3);
        let highlight = harness.state().ocr.highlight().unwrap();
        assert_eq!(&text[highlight], "第 4 行识别出的文字 Line 4");
        save(&mut harness, &format!("ocr-selected-{suffix}"));

        // 只选中这一行里的几个字：右边只标亮这几个字（汉字按字节算也对得上）。
        harness.state_mut().ocr.select_chars(3, 2, 6);
        harness.run_steps(3);
        let highlight = harness.state().ocr.highlight().unwrap();
        assert_eq!(&text[highlight], "4 行识");
        save(&mut harness, &format!("ocr-selected-part-{suffix}"));

        // ⇧⌘C 复制全部。
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::C);
        harness.run_steps(2);
        assert_eq!(
            harness.state().toast.as_ref().map(|t| t.text.as_str()),
            Some("已复制")
        );
        assert_eq!(
            uti::plain_text(&harness.state().pasteboard().read()).as_deref(),
            Some(text.as_str())
        );

        // 在右边的文字里输入：识别出的位置对不上，标亮消失。
        click_at(
            &mut harness,
            egui::pos2(600.0, 300.0),
            egui::PointerButton::Primary,
        );
        harness.event(egui::Event::Text("改".into()));
        harness.run_steps(3);
        assert_eq!(harness.state().ocr.highlight(), None);
        assert!(harness.state().ocr.text().unwrap().contains('改'));
        save(&mut harness, &format!("ocr-edited-{suffix}"));

        // 窗口拉大：左右两栏按原来的比例一起变宽，图片跟着重新适应栏宽（放得更大）。
        let fit = harness.state().ocr.fit();
        harness.set_size(egui::vec2(1120.0, 640.0));
        harness.run_steps(3);
        let wider = harness.state().ocr.fit();
        assert!(
            wider > fit * 1.1,
            "窗口变大后图片应当跟着放大：{fit} → {wider}"
        );
        save(&mut harness, &format!("ocr-wider-{suffix}"));

        // 没有识别到文字。
        harness
            .state_mut()
            .ocr
            .set_result(image.clone(), Some(Vec::new()));
        harness.run_steps(3);
        save(&mut harness, &format!("ocr-empty-{suffix}"));

        // esc 关闭。
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert_eq!(harness.state().page, Page::Panel);
    }
}

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_update_prompt() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    let release = Release {
        version: "0.2.0".into(),
        page_url: "https://github.com/zvae/devtools_hub/releases/tag/v0.2.0".into(),
        asset: Some(Asset {
            name: "DevTools-Hub-0.2.0-macos-arm64.dmg".into(),
            url: "https://github.com/zvae/devtools_hub/releases/download/v0.2.0/DevTools-Hub-0.2.0-macos-arm64.dmg".into(),
            size: 10_485_760,
            sha256: None,
        }),
    };
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        // 发现新版本：面板顶部显示提示。
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 640.0), |app| {
            app.shared.modify_update(|state| {
                state.status = Status::Available(release.clone());
                state.checked_at = Some(std::time::SystemTime::now());
            });
            app.panel.reset_for_show();
        });
        assert_eq!(harness.state().pending_update(), Some(release.clone()));
        save(&mut harness, &format!("update-panel-{suffix}"));

        // 菜单栏的“发现新版本”：打开设置页并滚动到底部的更新信息。
        // （测试程序不在 .app 里，不支持应用内安装，显示的是浏览器下载。）
        harness.state_mut().on_shown(Page::Settings);
        harness.state().shared.post(AppEvent::RevealUpdate);
        harness.run_steps(4);
        assert!(!harness.state().settings_view.reveal_update);
        save(&mut harness, &format!("update-settings-{suffix}"));

        // 正在下载：设置页显示进度条与取消按钮，面板提示条显示百分比。
        harness.state().shared.modify_update(|state| {
            state.install = Some(Install::Downloading {
                received: 3_565_158,
                total: 10_485_760,
            })
        });
        harness.run_steps(3);
        save(&mut harness, &format!("update-downloading-{suffix}"));
        harness.state_mut().page = Page::Panel;
        harness.run_steps(3);
        save(&mut harness, &format!("update-panel-downloading-{suffix}"));

        // 安装失败：显示原因，可以改用浏览器下载。
        harness.state().shared.modify_update(|state| {
            state.install = Some(Install::Failed(
                "复制新版本失败，请确认对 /Applications 有写入权限".into(),
            ))
        });
        harness.state_mut().reveal_update();
        harness.run_steps(4);
        save(&mut harness, &format!("update-install-failed-{suffix}"));
        harness
            .state()
            .shared
            .modify_update(|state| state.install = None);

        // 跳过此版本：面板与菜单栏不再提示，设置页仍显示下载按钮。
        harness.state_mut().settings.skipped_version = Some("0.2.0".into());
        harness.run_steps(2);
        assert_eq!(harness.state().pending_update(), None);
        save(&mut harness, &format!("update-skipped-{suffix}"));

        // 已是最新版本 / 手动检查失败。
        harness
            .state()
            .shared
            .modify_update(|state| state.status = Status::UpToDate);
        harness.run_steps(3);
        save(&mut harness, &format!("update-latest-{suffix}"));
        harness.state().shared.modify_update(|state| {
            state.status = Status::Failed("无法连接 GitHub，请检查网络".into());
            state.checking = true;
        });
        harness.run_steps(3);
        save(&mut harness, &format!("update-failed-{suffix}"));
    }
}

/// 搜索页离屏渲染：验证布局不 panic、结果行能画出来（输出 PNG 供人工检查）。
#[test]
#[ignore = "需要 GPU；输出 PNG 供人工检查"]
fn snapshot_search_page() {
    use std::time::{Duration, Instant};

    use devtools_search::application::protocol::Command as SearchCommand;
    use devtools_search::settings::{RootConfig, RootLocation, VolumeId};

    // 索引根：一个临时目录，里面放几个文件。
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("季度报告.pdf"), "pdf").unwrap();
    std::fs::write(root.path().join("notes.md"), "md").unwrap();
    std::fs::write(root.path().join("README.txt"), "readme").unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();

    let data = tempfile::tempdir().unwrap();
    let shared = Shared::new(&Settings::default());
    let mut app = HubApp::build(
        egui::Context::default(),
        shared,
        Store::open(data.path()).unwrap(),
        data.path().to_path_buf(),
        Settings::default(),
    );
    app.settings.search_enabled = true;
    app.sync_search_module();
    // 配置索引根并完成首次引导。
    {
        let module = app.search.as_mut().expect("已启用搜索");
        let mut draft = module.service().settings().clone();
        draft.permission_onboarding_completed = true;
        draft.lazy_start = false;
        draft.show_all_on_empty = false;
        draft.history_enabled = true;
        draft.roots = vec![RootConfig {
            location: RootLocation {
                volume_id: VolumeId::fallback_absolute(root.path()),
                relative_path: String::new(),
                fallback_absolute_path: root.path().to_path_buf(),
            },
            enabled: true,
        }];
        draft.exclude_patterns = vec!["node_modules".into()];
        module
            .service_mut()
            .handle(SearchCommand::ApplySettings {
                draft: Box::new(draft),
            });
    }
    // 等索引就绪。
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_search();
        let ready = app
            .search
            .as_ref()
            .map(|module| {
                module
                    .service()
                    .roots()
                    .iter()
                    .all(|root| matches!(root.state, devtools_search::RootState::Ready))
            })
            .unwrap_or(false);
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "索引没有就绪");
        std::thread::sleep(Duration::from_millis(10));
    }
    // 提交一次查询并等结果。
    if let Some(module) = app.search.as_mut() {
        module
            .service_mut()
            .handle(SearchCommand::Search {
                text: "r".into(),
                literal: false,
                case_sensitive: false,
                scope: None,
            });
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll_search();
        let len = app
            .search
            .as_ref()
            .map(|module| module.service().results_len())
            .unwrap_or(0);
        if len > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "查询没有结果");
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(module) = app.search.as_mut() {
        module.service_mut().handle(SearchCommand::LoadRows {
            start: 0,
            count: 128,
        });
    }
    // 入口带来的范围胶囊（模拟快捷面板选中文件夹后打开搜索）。
    if let Some(module) = app.search.as_mut() {
        module.on_shown(Some(devtools_search::SearchOptions {
            query: String::new(),
            selected_paths: vec![root.path().to_path_buf()],
            query_is_literal: false,
            case_sensitive: false,
        }));
    }
    app.poll_search();
    app.set_page(Page::Search);

    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 680.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut HubApp| {
                app.poll_search();
                app.draw(ui);
            },
            app,
        );
    let ctx = harness.ctx.clone();
    style::install(&ctx);
    ctx.set_theme(egui::Theme::Light);
    harness.state_mut().ctx = ctx;
    harness.run_steps(6);
    // 等到行数据到达后再截图。
    for _ in 0..100 {
        let has_rows = harness
            .state()
            .search
            .as_ref()
            .map(|module| module.service().results_len())
            .unwrap_or(0)
            > 0;
        if has_rows {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
    harness.run_steps(4);
    save(&mut harness, "search-page");

    // 模块内的设置页。
    if let Some(module) = harness.state_mut().search.as_mut() {
        module.show_settings();
    }
    harness.run_steps(4);
    save(&mut harness, "search-settings");

    // 暗色主题：确认取色跟随主题（§5.6）。
    let ctx = harness.ctx.clone();
    ctx.set_theme(egui::Theme::Dark);
    harness.state_mut().ctx = ctx;
    harness.run_steps(3);
    save(&mut harness, "search-settings-dark");
}

/// 首次使用的权限引导页（新数据目录、引导未完成）。
#[test]
#[ignore = "需要 GPU；输出 PNG 供人工检查"]
fn snapshot_search_permissions() {
    let data = tempfile::tempdir().unwrap();
    let mut app = HubApp::build(
        egui::Context::default(),
        Shared::new(&Settings::default()),
        Store::open(data.path()).unwrap(),
        data.path().to_path_buf(),
        Settings::default(),
    );
    app.settings.search_enabled = true;
    app.sync_search_module();
    app.set_page(Page::Search);

    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 680.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut HubApp| {
                app.poll_search();
                app.draw(ui);
            },
            app,
        );
    let ctx = harness.ctx.clone();
    style::install(&ctx);
    ctx.set_theme(egui::Theme::Light);
    harness.state_mut().ctx = ctx;
    harness.run_steps(4);
    save(&mut harness, "search-permissions");

    let ctx = harness.ctx.clone();
    ctx.set_theme(egui::Theme::Dark);
    harness.state_mut().ctx = ctx;
    harness.run_steps(3);
    save(&mut harness, "search-permissions-dark");
}

/// 空态：首次打开、索引尚未建立时的引导（大图标 + 主按钮）。
#[test]
#[ignore = "需要 GPU；输出 PNG 供人工检查"]
fn snapshot_search_empty() {
    use devtools_search::application::protocol::Command as SearchCommand;

    let data = tempfile::tempdir().unwrap();
    let mut app = HubApp::build(
        egui::Context::default(),
        Shared::new(&Settings::default()),
        Store::open(data.path()).unwrap(),
        data.path().to_path_buf(),
        Settings::default(),
    );
    app.settings.search_enabled = true;
    app.sync_search_module();
    // 完成引导但保持惰性启动：空态显示"索引尚未建立 + 立即建立索引"。
    if let Some(module) = app.search.as_mut() {
        let mut draft = module.service().settings().clone();
        draft.permission_onboarding_completed = true;
        draft.lazy_start = true;
        draft.show_all_on_empty = false;
        draft.history_enabled = true;
        module
            .service_mut()
            .handle(SearchCommand::ApplySettings {
                draft: Box::new(draft),
            });
    }
    app.poll_search();
    app.set_page(Page::Search);

    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 680.0))
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut HubApp| {
                app.poll_search();
                app.draw(ui);
            },
            app,
        );
    let ctx = harness.ctx.clone();
    style::install(&ctx);
    ctx.set_theme(egui::Theme::Light);
    harness.state_mut().ctx = ctx;
    harness.run_steps(4);
    save(&mut harness, "search-empty");
}
