//! 离屏渲染测试：用示例数据渲染面板和设置页，输出 PNG 供人工检查，并验证键盘操作。
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
use egui_kittest::Harness;
use image::{ImageFormat, Rgba, RgbaImage};

use super::{style, HubApp, Page};
use crate::settings::Settings;
use crate::shared::Shared;

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
        harness.set_size(egui::vec2(380.0, 1500.0));
        harness.run_steps(3);
        save(&mut harness, &format!("settings-full-{suffix}"));
    }
}
