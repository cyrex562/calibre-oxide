/// Minimal ICU stubs
/// In the future, this should interface with proper ICU libraries.
/// For now, we use Rust Standard Library unicode methods.

pub fn lower(text: &str) -> String {
    text.to_lowercase()
}

pub fn upper(text: &str) -> String {
    text.to_uppercase()
}

pub fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
