//! 离屏渲染测试：用示例数据渲染面板、设置页、鼠标中键快捷面板和 JSON 编辑器，
//! 输出 PNG 供人工检查，并验证键盘操作。
//!
//! 需要 GPU（wgpu/Metal），默认忽略。手动运行：
//! `cargo test -p devtools-hub snapshot -- --ignored`
//! 输出目录：`target/ui-snapshots/`（可用环境变量 `UI_SNAPSHOT_DIR` 覆盖）。

use std::io::Cursor;
use std::path::PathBuf;

use devtools_clipboard::store::now_ms;
use devtools_clipboard::{
    thumbnail, uti, Capture, Representation, Snapshot, SourceApp, Store, FAVORITES_LIST_ID,
};
use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use image::{ImageFormat, Rgba, RgbaImage};

use super::{style, HubApp, Page};
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
    let code = record(
        store,
        Snapshot::from_text(
            "fn main() {\n    let history = Store::open(dir)?;\n    println!(\"{} items\", history.len());\n}\n",
        ),
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
fn snapshot_context_menu() {
    let dir = tempfile::tempdir().unwrap();
    seed(&mut Store::open(dir.path()).unwrap());
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut harness = harness_for(dir.path(), theme, egui::vec2(380.0, 640.0), |app| {
            app.panel.reset_for_show();
        });
        // 右键最后一条（文件）：菜单里多出“在访达中显示”。
        click_at(
            &mut harness,
            egui::pos2(190.0, MENU_CARD_Y),
            egui::PointerButton::Secondary,
        );
        assert_eq!(harness.state().panel.selected, 5);
        save(&mut harness, &format!("menu-{suffix}"));

        // 悬停“删除”：红底白字。
        harness.get_by_label("删除").hover();
        harness.run_steps(4);
        save(&mut harness, &format!("menu-delete-{suffix}"));

        // 点击“添加到列表”：在菜单内翻到列表页，已加入的列表打勾。
        harness.get_by_label("添加到列表").click();
        harness.run_steps(4);
        assert!(harness.query_by_label("新建").is_some());
        save(&mut harness, &format!("menu-lists-{suffix}"));

        // 在菜单的输入框里打字：空格、回车不触发面板的预览与粘贴，回车新建列表并加入条目。
        // 第一个输入框是搜索框，菜单里的输入框在后面。
        let field = |harness: &Harness<'_, HubApp>| {
            let fields: Vec<_> = harness
                .get_all_by_role(egui::accesskit::Role::TextInput)
                .collect();
            assert_eq!(fields.len(), 2);
            fields[1].rect().center()
        };
        let pos = field(&harness);
        click_at(&mut harness, pos, egui::PointerButton::Primary);
        harness.event(egui::Event::Text("工作 笔记".into()));
        harness.run_steps(2);
        assert!(!harness.state().panel.preview_open);
        save(&mut harness, &format!("menu-new-list-{suffix}"));
        harness.key_press(Key::Enter);
        harness.run_steps(4);
        let panel = &harness.state().panel;
        assert!(!egui::Popup::is_any_open(&harness.ctx), "新建后菜单应关闭");
        let list = panel
            .lists
            .iter()
            .find(|l| l.name == "工作 笔记")
            .expect("应新建列表");
        assert!(panel.items[5].in_list(list.id));
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

        // 右键“链接”标签 → 隐藏此标签。
        click_at(&mut harness, LINK_CHIP, egui::PointerButton::Secondary);
        save(&mut harness, &format!("tab-menu-{suffix}"));
        harness.get_by_label("隐藏此标签").click();
        harness.run_steps(4);
        let tabs = harness.state().settings.tabs.clone();
        assert!(tabs.contains(&TabSetting {
            tab: Tab::Link,
            visible: false,
        }));
        save(&mut harness, &format!("tabs-hidden-{suffix}"));

        // 右键“全部” → 编辑分类标签…：打开设置页并滚动到“分类标签”分组。
        click_at(&mut harness, ALL_CHIP, egui::PointerButton::Secondary);
        harness.get_by_label("编辑分类标签…").click();
        harness.run_steps(5);
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

/// 380×640 的面板里，分类标签一行的控件位置。
const ALL_CHIP: egui::Pos2 = egui::pos2(39.0, 68.0);
const LINK_CHIP: egui::Pos2 = egui::pos2(139.0, 68.0);
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
            app.quick.open(None);
            app.page = Page::QuickActions;
            probe = Some(quick_size(app));
        });
        harness.set_size(probe.unwrap());
        harness.run_steps(2);
        save(&mut harness, &format!("quick-empty-{suffix}"));

        // 选中的文本不是 JSON：提示没有匹配的功能。
        let mut harness = harness_for(dir.path(), theme, egui::vec2(300.0, 200.0), |app| {
            app.quick
                .open(Some(Selection::Text("今天的会议改到下午三点".into())));
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
            app.quick.open(Some(Selection::Text(SELECTED_JSON.into())));
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
