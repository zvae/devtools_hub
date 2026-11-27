//! JSON 编辑器原有的格式化、键盘、过滤及错误展示测试。
use super::*;
use egui::{Key, Modifiers};
const SELECTED_JSON: &str = r#"{"menuId": 701011,"menuName": "预算和投入","menuNameEn": "","parentId": 7010,"icon": "","routePath": "bugetInvest","sort": 2.0,"visible": true,"subList": [{"menuId": 70101101, "menuName": "预算编制", "todoCount": 3}],"todoCount": 0}"#;
#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_json_editor() {
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut tool = Tool {
            json: JsonEditorView::default(),
            indent: Indent::default(),
            palette: style::Palette::new(true),
            effects: Vec::new(),
        };
        tool.activate(Some(devtools_module_api::Selection::Text(
            SELECTED_JSON.into(),
        )));
        let mut harness = crate::test_support::harness(tool, theme, egui::vec2(760.0, 560.0));
        assert!(harness
            .state()
            .tool
            .json
            .text
            .starts_with("{\n    \"menuId\": 701011,\n    \"menuName\": \"预算和投入\","));
        assert!(
            harness.state().tool.json.text.contains("\"sort\": 2.0,"),
            "数字保持原样"
        );
        harness.set_size(egui::vec2(760.0, 560.0));
        harness.run_steps(4);
        crate::test_support::save(&mut harness, &format!("json-editor-{suffix}"));

        // ⌥C 压缩并复制。
        harness.key_press_modifiers(Modifiers::ALT, Key::C);
        harness.run_steps(2);
        assert_eq!(
            harness.state().toast.as_deref(),
            Some("已复制压缩后的 JSON")
        );
        assert!(
            !harness.state().tool.json.text.contains('ç'),
            "⌥C 不应输入字符"
        );

        // 路径过滤。
        harness.state_mut().tool.json.filter = ".subList[].menuName".into();
        harness.run_steps(4);
        crate::test_support::save(&mut harness, &format!("json-filter-{suffix}"));

        // 格式错误：底栏显示行列号，出错行标红。
        harness.state_mut().tool.json.filter.clear();
        harness.state_mut().tool.json.text = "{\n    \"a\": 1\n    \"b\": [1, 2,]\n}".into();
        harness.run_steps(4);
        crate::test_support::save(&mut harness, &format!("json-error-{suffix}"));

        // esc 关闭编辑器。
        harness.key_press(Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().closed);
    }
}
