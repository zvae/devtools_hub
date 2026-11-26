//! 原有工具交互与离屏渲染测试，状态留在工具模块内部。
use super::*;
use egui::{Key, Modifiers};

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_timestamp() {
    use egui_kittest::kittest::Queryable;
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut tool = Tool {
            timestamp: TimestampView::default(),
            palette: style::Palette::new(true),
            effects: Vec::new(),
        };
        tool.activate(Some(devtools_module_api::Selection::Text(
            "1700000000123".into(),
        )));
        let mut harness = crate::test_support::harness(tool, theme, egui::vec2(680.0, 570.0));
        assert_eq!(harness.state().tool.timestamp.input, "1700000000123");
        harness.state_mut().tool.timestamp.running = false;
        harness.state_mut().tool.timestamp.now_ms = 1700000000123;
        harness.state_mut().tool.timestamp.date = "2023-11-15 06:13:20.123".into();
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("timestamp-{suffix}"));
        harness
            .get(
                egui_kittest::kittest::By::new()
                    .value("1700000000123")
                    .role(egui::accesskit::Role::TextInput),
            )
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(2);
        crate::test_support::save(&mut harness, &format!("timestamp-input-focus-{suffix}"));
        harness.get_by_label("格式说明 ⓘ").hover();
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("timestamp-help-{suffix}"));
        // 点击时钟带入并复制，再验证暂停／继续不会清空编辑中的输入。
        harness.state_mut().tool.timestamp.input = "0".into();
        harness.get_by_label("1700000000123").click();
        harness.run_steps(2);
        assert_eq!(harness.state().tool.timestamp.input, "1700000000123");
        assert_eq!(harness.state().toast.as_deref(), Some("已复制"));
        harness.get_by_label("继续").click();
        harness.run_steps(2);
        assert!(harness.state().tool.timestamp.running);
        harness.get_by_label("暂停").click();
        harness.run_steps(2);
        assert!(!harness.state().tool.timestamp.running);
        let paused = harness.state().tool.timestamp.now_ms;
        harness.run_steps(2);
        assert_eq!(harness.state().tool.timestamp.now_ms, paused);
        assert_eq!(harness.state().tool.timestamp.input, "1700000000123");
        harness.state_mut().tool.timestamp.now_ms = 1700000000123;
        harness.state_mut().toast = None;
        harness.get_by_label("日期 → 时间戳").click();
        harness.run_steps(2);
        assert!(harness.state().tool.timestamp.date_to_timestamp);
        crate::test_support::save(&mut harness, &format!("timestamp-date-{suffix}"));
        harness
            .get(
                egui_kittest::kittest::By::new()
                    .value("2023-11-15 06:13:20.123")
                    .role(egui::accesskit::Role::TextInput),
            )
            .click();
        harness.run_steps(2);
        crate::test_support::save(&mut harness, &format!("timestamp-date-focus-{suffix}"));
        harness.get_by_label("时间戳 → 日期").click();
        harness.run_steps(2);
        assert!(!harness.state().tool.timestamp.date_to_timestamp);
        harness.state_mut().tool.timestamp.input = "invalid".into();
        harness.run_steps(2);
        crate::test_support::save(&mut harness, &format!("timestamp-error-{suffix}"));
        harness.set_size(egui::vec2(380.0, 600.0));
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("timestamp-narrow-{suffix}"));
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().closed);
    }
}
