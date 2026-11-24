use unicode_width::UnicodeWidthStr;

/// Returns the width of a character as displayed in a wide terminal.
/// Double width characters (East Asian Wide) are counted as 2.
pub fn chr_width(c: char) -> usize {
    // unicode-width's width_cjk returns 2 for Wide/Fullwidth, 1 for others (mostly).
    // It returns None for things like control characters, which we'll treat as 0 or 1?
    // The python code: 1 + eaw(x).startswith('W')
    // eaw return values:
    // 'Na' (Narrow), 'H' (Halfwidth), 'N' (Neutral), 'A' (Ambiguous) -> 1
    // 'W' (Wide), 'F' (Fullwidth) -> 2
    // unicode_width::width_cjk behaves similarly for W/F -> 2.
    unicode_width::UnicodeWidthChar::width_cjk(c).unwrap_or(0)
}

/// Returns the width of the string when displayed in a terminal.
pub fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width_cjk(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_width() {
        assert_eq!(str_width("abc"), 3);
        assert_eq!(str_width("你好"), 4); // CJK characters are usually wide
        assert_eq!(str_width("a你好b"), 6);
    }
}
