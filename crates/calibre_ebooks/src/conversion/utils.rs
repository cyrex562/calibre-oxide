use anyhow::Result;

pub fn clean_ascii_chars(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii()).collect()
}

pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .replace(" ", "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

pub fn split_path(path: &str) -> Vec<String> {
    path.split(|c| c == '/' || c == '\\')
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
