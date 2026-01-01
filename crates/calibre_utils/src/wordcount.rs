/// Wordcount utilities

pub struct WordCount {
    pub characters: usize,
    pub chars_no_spaces: usize,
    pub asian_chars: usize,
    pub non_asian_words: usize,
    pub words: usize,
}

const IDEOGRAPHIC_SPACE: char = '\u{3000}';

fn is_asian(c: char) -> bool {
    c as u32 > IDEOGRAPHIC_SPACE as u32
}

fn filter_jchars(c: char) -> char {
    if is_asian(c) { ' ' } else { c }
}

fn nonj_len(text: &str) -> usize {
    let filtered: String = text.chars().map(filter_jchars).collect();
    filtered.split_whitespace().count()
}

pub fn get_wordcount(text: &str) -> WordCount {
    let characters = text.chars().count(); // Python len(text) is char count (in unicode string)
    let chars_no_spaces = text.chars().filter(|c| !c.is_whitespace()).count();
    let asian_chars = text.chars().filter(|&c| is_asian(c)).count();
    let non_asian_words = nonj_len(text);
    let words = non_asian_words + asian_chars;
    
    WordCount {
        characters,
        chars_no_spaces,
        asian_chars,
        non_asian_words,
        words
    }
}
