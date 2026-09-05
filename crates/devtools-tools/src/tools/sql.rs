use super::*;

pub(super) fn format_sql(input: &str) -> CommandResult {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut formatted = normalized;
    for keyword in [
        "SELECT",
        "FROM",
        "WHERE",
        "GROUP BY",
        "ORDER BY",
        "HAVING",
        "LIMIT",
        "LEFT JOIN",
        "RIGHT JOIN",
        "INNER JOIN",
        "JOIN",
        "UNION",
    ] {
        formatted = replace_case_insensitive(&formatted, keyword, &format!("\n{keyword}"));
    }
    CommandResult {
        title: "SQL Format".into(),
        content: formatted.trim_start().replace(", ", ",\n  "),
    }
}

pub(super) fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut rest = input;
    let lower_needle = needle.to_ascii_lowercase();
    while let Some(index) = rest.to_ascii_lowercase().find(&lower_needle) {
        result.push_str(&rest[..index]);
        result.push_str(replacement);
        rest = &rest[index + needle.len()..];
    }
    result.push_str(rest);
    result
}
