//! 鼠标中键快捷面板里的功能。
//!
//! uTools 由插件在 `plugin.json` 中声明功能及其匹配规则（features / cmds），面板按选中内容筛选。
//! 例如「JSON 编辑器」插件声明了
//! `{"type":"regex","minLength":2,"match":"/^\s*(\{[\s\S]*\}|\[[\s\S]*\])\s*$/","label":"Json"}`
//! 与 `{"type":"files","fileType":"file","maxLength":1,"match":"/\.json$/i","label":"Json"}`。
//! 这里功能是内置的，匹配规则直接写成代码。
//!
//! 匹配规则和功能名在这里（依赖选中内容，属于界面编排）；JSON 的处理在
//! [`devtools_json`] 里，不依赖界面与平台。

pub use devtools_json as json;

use crate::selection::Selection;

/// 快捷面板里的一个功能。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickAction {
    /// 用 JSON 编辑器打开选中的 JSON（或 .json 文件）；没有选中内容时直接打开编辑器。
    JsonEditor,
    /// 打开剪贴板历史面板。
    ClipboardHistory,
    /// 文件搜索：把选中的文本/路径带到搜索页。
    FileSearch,
}

impl QuickAction {
    /// 功能名称。
    pub fn label(self) -> &'static str {
        match self {
            QuickAction::JsonEditor => "JSON 编辑器",
            QuickAction::ClipboardHistory => "剪贴板历史",
            QuickAction::FileSearch => "文件搜索",
        }
    }

    /// 名称右侧的简短说明。
    pub fn description(self) -> &'static str {
        match self {
            QuickAction::JsonEditor => "格式化、压缩、转义、路径过滤",
            QuickAction::ClipboardHistory => "查看并粘贴复制过的内容",
            QuickAction::FileSearch => "按文件名、路径查找本机文件",
        }
    }

    /// 是否能处理选中的内容。
    fn matches(self, selection: &Selection) -> bool {
        match (self, selection) {
            (QuickAction::JsonEditor, Selection::Text(text)) => json::looks_like_json(text),
            (QuickAction::JsonEditor, Selection::Files(paths)) => {
                matches!(paths.as_slice(), [path] if is_json_file(path))
            }
            (QuickAction::ClipboardHistory, _) => false,
            // 文件搜索能处理任何选中内容：文本按字面短语搜，路径作为范围。
            (QuickAction::FileSearch, _) => true,
        }
    }
}

/// 按选中内容匹配的功能。
const CONTENT_ACTIONS: [QuickAction; 1] = [QuickAction::JsonEditor];

/// 面板里列出的功能：能处理选中内容的排在前面，最后是总是可用的“剪贴板历史”。
/// 没有选中内容时列出所有功能（对应 uTools 未选中时显示的常用功能）。
/// 文件搜索按开关提供（关闭时不在面板里出现，与 §4.3 的入口语义一致）。
pub fn actions_for(selection: Option<&Selection>, search_enabled: bool) -> Vec<QuickAction> {
    let mut actions: Vec<QuickAction> = CONTENT_ACTIONS
        .into_iter()
        .filter(|action| selection.is_none_or(|selection| action.matches(selection)))
        .collect();
    if search_enabled {
        actions.push(QuickAction::FileSearch);
    }
    actions.push(QuickAction::ClipboardHistory);
    actions
}

/// 是否有功能能处理选中的内容（没有时面板显示提示）。
pub fn has_match(selection: &Selection, search_enabled: bool) -> bool {
    CONTENT_ACTIONS
        .iter()
        .any(|action| action.matches(selection))
        || search_enabled
}

/// 扩展名是 .json（不区分大小写）的已存在文件。
fn is_json_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        && std::path::Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_actions_by_selection() {
        assert_eq!(
            actions_for(None, false),
            [QuickAction::JsonEditor, QuickAction::ClipboardHistory]
        );
        assert_eq!(
            actions_for(None, true),
            [
                QuickAction::JsonEditor,
                QuickAction::FileSearch,
                QuickAction::ClipboardHistory
            ]
        );
        let json = Selection::Text("{\"a\": 1}".into());
        assert_eq!(
            actions_for(Some(&json), false),
            [QuickAction::JsonEditor, QuickAction::ClipboardHistory]
        );
        assert!(has_match(&json, false));
        let text = Selection::Text("hello".into());
        assert_eq!(actions_for(Some(&text), false), [QuickAction::ClipboardHistory]);
        assert!(!has_match(&text, false));
        // 开启文件搜索后，任何文本都能搜索，面板不再显示"没有能处理的功能"。
        assert!(has_match(&text, true));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.JSON");
        std::fs::write(&file, "{}").unwrap();
        let file = file.to_string_lossy().into_owned();
        assert!(has_match(&Selection::Files(vec![file.clone()]), false));
        assert!(!has_match(&Selection::Files(vec![file.clone(), file]), false));
        assert!(!has_match(&Selection::Files(vec!["/nope/a.json".into()]), false));
    }
}
