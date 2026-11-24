use unicode_names2;

pub fn character_name_from_code(code: u32) -> String {
    if let Some(c) = std::char::from_u32(code) {
        if let Some(name) = unicode_names2::name(c) {
            return name.to_string();
        }
    }
    format!("U+{:X}", code)
}

pub fn points_for_word(word: &str) -> Vec<u32> {
    let word = word.to_uppercase();
    // This is expensive: iterating all chars. But implementation plan accepted it.
    // Optimization would be a trie or reverse index.
    let mut points = Vec::new();
    // Only iterate typical unicode range?
    for i in 0..0x110000 {
        if let Some(c) = std::char::from_u32(i) {
             if let Some(name) = unicode_names2::name(c) {
                 if name.to_string().contains(&word) {
                     points.push(i);
                 }
             }
        }
    }
    points
}
