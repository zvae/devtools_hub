//! 原有工具交互与离屏渲染测试，状态留在工具模块内部。
use super::*;
use egui::Key;

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_password_generator() {
    use egui_kittest::kittest::Queryable;
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut tool = Tool {
            password: PasswordView::default(),
            palette: style::Palette::new(true),
            effects: Vec::new(),
        };
        tool.activate(None);
        let mut harness = crate::test_support::harness(tool, theme, egui::vec2(680.0, 560.0));
        assert_eq!(harness.state().tool.password.value.len(), 12);
        assert!(harness.state().tool.password.options.numbers);
        assert!(harness.state().tool.password.options.symbols);
        // 快照使用固定的展示样例，不输出随机生成的真实结果。
        harness.state_mut().tool.password.value = "vK7!mR2@nT9#".into();
        harness.run_steps(2);
        crate::test_support::save(&mut harness, &format!("password-{suffix}"));
        harness.get_by_label("复制密码").click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().copied.as_ref(),
            Some(&("vK7!mR2@nT9#".into(), true))
        );
        harness.get_by_label("包含数字").click();
        harness.run_steps(2);
        assert!(!harness.state().tool.password.options.numbers);
        assert!(!harness
            .state()
            .tool
            .password
            .value
            .bytes()
            .any(|b| b.is_ascii_digit()));
        harness.get_by_label("包含符号").click();
        harness.run_steps(2);
        assert!(!harness.state().tool.password.options.symbols);
        assert!(harness
            .state()
            .tool
            .password
            .value
            .bytes()
            .all(|b| b.is_ascii_alphabetic()));
        harness.get_by_label("包含符号").click();
        harness.run_steps(2);
        assert!(harness.state().tool.password.options.symbols);
        assert!(harness
            .state()
            .tool
            .password
            .value
            .bytes()
            .any(|b| b.is_ascii_punctuation()));
        harness.state_mut().tool.password.options.length = 100;
        harness.get_by_label("重新生成").click();
        harness.run_steps(2);
        assert_eq!(harness.state().tool.password.value.len(), 100);
        harness.state_mut().tool.password.value = "aB!xY#cD$mN%pQ&kR*sT".repeat(5);
        harness.state_mut().toast = None;
        harness.run_steps(2);
        crate::test_support::save(&mut harness, &format!("password-long-{suffix}"));
        harness.set_size(egui::vec2(380.0, 640.0));
        harness.run_steps(3);
        crate::test_support::save(&mut harness, &format!("password-narrow-{suffix}"));
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().closed);
    }
}
