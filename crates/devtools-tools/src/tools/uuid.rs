use super::*;

pub(super) fn generate_uuids(input: &str) -> CommandResult {
    let count = input.trim().parse::<usize>().unwrap_or(1).clamp(1, 100);
    let content = (0..count)
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    CommandResult {
        title: "UUID Generator".into(),
        content,
    }
}

pub(super) fn normalize_uuid(input: &str) -> CommandResult {
    let compact = input.trim().replace('-', "");
    let content = match uuid::Uuid::parse_str(&compact) {
        Ok(value) => format!(
            "Lowercase: {}\nUppercase: {}\nCompact: {}",
            value.hyphenated(),
            value.hyphenated().to_string().to_ascii_uppercase(),
            value.simple()
        ),
        Err(error) => format!("Invalid UUID: {error}"),
    };
    CommandResult {
        title: "UUID Normalize".into(),
        content,
    }
}
