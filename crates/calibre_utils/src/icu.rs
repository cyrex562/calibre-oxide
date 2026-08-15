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

/// Port of `calibre.utils.icu.title_case`: uppercase the first letter of
/// each word, lowercase the rest. A word boundary is any non-alphabetic
/// character. Narrower than real ICU title-casing (which uses full
/// Unicode word-break rules and a small exception list for
/// articles/prepositions in some locales), but correct for the common
/// case this crate uses it for (CSS `text-transform: capitalize`).
pub fn title_case(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if at_word_start {
                result.extend(ch.to_uppercase());
                at_word_start = false;
            } else {
                result.extend(ch.to_lowercase());
            }
        } else {
            result.push(ch);
            at_word_start = true;
        }
    }
    result
}
