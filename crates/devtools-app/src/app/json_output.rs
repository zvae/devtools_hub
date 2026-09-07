use std::collections::HashSet;

use devtools_ui::{JsonLineView, ToolWindow};
use slint::{ModelRc, VecModel};

pub(super) struct JsonDocument {
    lines: Vec<String>,
    ends: Vec<Option<usize>>,
    collapsed: HashSet<usize>,
}

impl JsonDocument {
    pub(super) fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        if !value.is_object() && !value.is_array() {
            return None;
        }
        let formatted = serde_json::to_string_pretty(&value).ok()?;
        let lines: Vec<String> = formatted.lines().map(str::to_owned).collect();
        let mut ends = vec![None; lines.len()];
        let mut stack = Vec::new();
        // The serializer puts nonempty containers on separate lines. Ignore delimiters
        // inside escaped strings, including object keys containing brackets or quotes.
        let mut quoted = false;
        let mut escaped = false;
        for (index, line) in lines.iter().enumerate() {
            for ch in line.chars() {
                if quoted {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        quoted = false;
                    }
                } else {
                    match ch {
                        '"' => quoted = true,
                        '{' | '[' => stack.push(index),
                        '}' | ']' => {
                            let start = stack.pop()?;
                            if start != index {
                                ends[start] = Some(index);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Some(Self {
            lines,
            ends,
            collapsed: HashSet::new(),
        })
    }

    pub(super) fn toggle(&mut self, line: usize) {
        if self.ends.get(line).is_some_and(Option::is_some) && !self.collapsed.remove(&line) {
            self.collapsed.insert(line);
        }
    }

    pub(super) fn set_all_collapsed(&mut self, collapsed: bool) {
        self.collapsed.clear();
        if collapsed {
            self.collapsed.extend(
                self.ends
                    .iter()
                    .enumerate()
                    .filter_map(|(i, end)| end.map(|_| i)),
            );
        }
    }

    fn rows(&self) -> Vec<JsonLineView> {
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.lines.len() {
            let collapsed = self.collapsed.contains(&index);
            let text = if collapsed {
                let end = self.ends[index].expect("only containers can be collapsed");
                format!("{} ... {}", self.lines[index], self.lines[end].trim_start())
            } else {
                self.lines[index].clone()
            };
            rows.push(JsonLineView {
                line_id: index as i32,
                number: (index + 1).to_string().into(),
                text: text.into(),
                expandable: self.ends[index].is_some(),
                collapsed,
            });
            index = if collapsed {
                self.ends[index].unwrap() + 1
            } else {
                index + 1
            };
        }
        rows
    }

    pub(super) fn update_window(&self, window: &ToolWindow) {
        let rows = self.rows();
        window.set_json_display_text(
            rows.iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .into(),
        );
        window.set_json_lines(ModelRc::new(VecModel::from(rows)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_folds_keep_original_line_numbers_and_child_state() {
        let mut doc = JsonDocument::parse(r#"{"a":[{"b":1}],"z":true}"#).unwrap();
        let before = doc.rows();
        doc.toggle(2);
        doc.toggle(1);
        let rows = doc.rows();
        assert_eq!(
            rows.iter().map(|r| r.number.as_str()).collect::<Vec<_>>(),
            ["1", "2", "7", "8"]
        );
        assert_eq!(rows[1].text.as_str(), "  \"a\": [ ... ],");
        doc.toggle(1);
        assert!(doc.rows()[2].collapsed);
        doc.set_all_collapsed(false);
        assert_eq!(doc.rows(), before);
    }

    #[test]
    fn brackets_and_escaped_quotes_in_strings_are_not_containers() {
        let doc =
            JsonDocument::parse(r#"{"[\\\"{": "}\\\"]", "empty": [], "object": {}}"#).unwrap();
        assert_eq!(doc.ends.iter().flatten().count(), 1);
    }

    #[test]
    fn collapse_all_and_expand_all_cover_arrays_and_objects() {
        let mut doc = JsonDocument::parse(r#"[{"a":[1,2]},{}]"#).unwrap();
        let count = doc.rows().len();
        doc.set_all_collapsed(true);
        assert_eq!(doc.rows().len(), 1);
        doc.set_all_collapsed(false);
        assert_eq!(doc.rows().len(), count);
        doc.toggle(usize::MAX);
        assert_eq!(doc.rows().len(), count);
    }

    #[test]
    fn invalid_and_scalar_outputs_use_plain_editor() {
        for text in ["", "Error: invalid JSON", "{", "true", "123", "\"text\""] {
            assert!(JsonDocument::parse(text).is_none());
        }
        for text in ["{}", "[]"] {
            assert!(!JsonDocument::parse(text).unwrap().rows()[0].expandable);
        }
    }
}
