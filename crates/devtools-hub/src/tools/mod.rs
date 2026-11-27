//! 快捷入口只区分宿主动作和通用工具，工具元数据来自编译期发现结果。

use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickAction {
    Tool(&'static str),
    ClipboardHistory,
    FileSearch,
}

impl QuickAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tool(id) => devtools_tools::descriptor(id).map_or("未知工具", |d| d.name),
            Self::ClipboardHistory => "剪贴板历史",
            Self::FileSearch => "文件搜索",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Tool(id) => devtools_tools::descriptor(id).map_or("", |d| d.description),
            Self::ClipboardHistory => "查看并粘贴复制过的内容",
            Self::FileSearch => "按文件名、路径查找本机文件",
        }
    }
}

pub fn actions_for(selection: Option<&Selection>, search_enabled: bool) -> Vec<QuickAction> {
    let mut actions: Vec<_> = devtools_tools::registrations()
        .iter()
        .filter(|d| selection.is_none_or(|input| (d.matches)(input)))
        .map(|d| QuickAction::Tool(d.id))
        .collect();
    if search_enabled {
        actions.push(QuickAction::FileSearch);
    }
    actions.push(QuickAction::ClipboardHistory);
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quick_actions_are_discovered_and_matched_without_host_tool_branches() {
        assert_eq!(
            actions_for(None, false).len(),
            devtools_tools::registrations().len() + 1
        );
        assert_eq!(
            actions_for(Some(&Selection::Text("{}".into())), false),
            [
                QuickAction::Tool("devtools.json"),
                QuickAction::ClipboardHistory
            ]
        );
        assert_eq!(
            actions_for(Some(&Selection::Text("1700000000".into())), false),
            [
                QuickAction::Tool("devtools.timestamp"),
                QuickAction::ClipboardHistory
            ]
        );
        assert_eq!(
            actions_for(Some(&Selection::Text("hello".into())), true),
            [QuickAction::FileSearch, QuickAction::ClipboardHistory]
        );
        // 已编码的文本只带出对应的编码工具，普通文本不会被误判。
        assert_eq!(
            actions_for(Some(&Selection::Text("5Lit5paH".into())), false),
            [
                QuickAction::Tool("devtools.base64"),
                QuickAction::ClipboardHistory
            ]
        );
        assert_eq!(
            actions_for(Some(&Selection::Text("%E4%B8%AD%E6%96%87".into())), false),
            [
                QuickAction::Tool("devtools.url"),
                QuickAction::ClipboardHistory
            ]
        );
        assert_eq!(
            actions_for(Some(&Selection::Text("\\xE4\\xB8\\xAD".into())), false),
            [
                QuickAction::Tool("devtools.utf8"),
                QuickAction::ClipboardHistory
            ]
        );
    }
}
