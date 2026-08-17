use fancy_regex::Regex as FancyRegex;
use lazy_static::lazy_static;
use regex::Regex;

// Helper to compile regexes
//
// A number of `educateQuotes`' patterns (ported from
// `old_src/src/calibre/utils/smartypants.py`) use lookaround
// (`(?=...)`/`(?!...)`), which the plain `regex` crate (used
// elsewhere in this file, where lookaround isn't needed) can't
// compile at all -- it would panic on first use. Those patterns use
// `fancy_regex` instead; see `educate_quotes` below for which crate
// each pattern needs.
lazy_static! {
    static ref TAGS_TO_SKIP_REGEX: Regex = Regex::new(r"(?i)<(/?)(style|pre|code|kbd|script|math)[^>]*>").unwrap();
    static ref SELF_CLOSING_REGEX: Regex = Regex::new(r"/\s*>$").unwrap();

    // Quotes regexes
    static ref OPENING_SINGLE_QUOTES_REGEX: FancyRegex = FancyRegex::new(r"(?x)
            (
                \s          |   # a whitespace char, or
                &nbsp;      |   # a non-breaking space entity, or
                --          |   # dashes, or
                &[mn]dash;  |   # named dash entities
                &#8211;|&#8212;          |   # or decimal entities
                &\#x201[34];    # or hex
            )
            '                 # the quote
            (?=\w)            # followed by a word character
            ").unwrap();

    static ref CLOSING_SINGLE_QUOTES_REGEX1: FancyRegex = FancyRegex::new(r"(?x)
            ([^\ \t\r\n\[\{\(\-])
            '
            (?!\s | s\b | \d)
            ").unwrap();

    static ref CLOSING_SINGLE_QUOTES_REGEX2: Regex = Regex::new(r"(?x)
            ([^\ \t\r\n\[\{\(\-])
            '
            (\s | s\b)
            ").unwrap();

    static ref OPENING_DOUBLE_QUOTES_REGEX: FancyRegex = FancyRegex::new(r"(?x)
            (
                \s          |   # a whitespace char, or
                &nbsp;      |   # a non-breaking space entity, or
                --          |   # dashes, or
                &[mn]dash;  |   # named dash entities
                &#8211;|&#8212;          |   # or decimal entities
                &\#x201[34];    # or hex
            )
            \x22                 # the quote
            (?=\w)            # followed by a word character
            ").unwrap();

    static ref CLOSING_DOUBLE_QUOTES_REGEX1: FancyRegex = FancyRegex::new(r#"(?x)
            "
            (?=\s)
            "#).unwrap();

    static ref CLOSING_DOUBLE_QUOTES_REGEX2: Regex = Regex::new(r#"(?x)
            ([^\ \t\r\n\[\{\(\-])   # character that indicates the quote should be closing
            "
            "#).unwrap();

    // The leading-quote-followed-by-punctuation, double-quote-set, and
    // decade-abbreviation patterns near the top of `educate_quotes`
    // (`^'(?={punct}\B)`, `"'(?=\w)`, `(\W|^)'(?=\d{{2}}s)`, ...) and
    // the "quotes nested inside other entities" patterns
    // (`(?<=\W)"(?=\w)` etc.) all need lookaround too.
    static ref RE_LEADING_SINGLE_QUOTE: FancyRegex =
        FancyRegex::new(&format!(r"(?m)^'(?={}\B)", *PUNCT_CLASS)).unwrap();
    static ref RE_LEADING_DOUBLE_QUOTE: FancyRegex =
        FancyRegex::new(&format!(r#"(?m)^"(?={}\B)"#, *PUNCT_CLASS)).unwrap();
    static ref RE_DOUBLE_SINGLE_OPEN: FancyRegex = FancyRegex::new(r#""'(?=\w)"#).unwrap();
    static ref RE_SINGLE_DOUBLE_OPEN: FancyRegex = FancyRegex::new(r#"'"(?=\w)"#).unwrap();
    static ref RE_DOUBLE_DOUBLE_OPEN: FancyRegex = FancyRegex::new(r#"""(?=\w)"#).unwrap();
    static ref RE_SINGLE_SINGLE_OPEN: FancyRegex = FancyRegex::new(r"''(?=\w)").unwrap();
    static ref RE_DECADE: FancyRegex = FancyRegex::new(r"(\W|^)'(?=\d{2}s)").unwrap();
    // NB: this pattern ends with a literal `"` (the inches mark, e.g.
    // `19' 43.5"`) -- the Python source has it too
    // (`r'''...(\s*[-0-9.]+)"'''`). An earlier version of this port
    // used `r#"...+)"#` for this pattern, whose closing `"#` swallowed
    // that trailing literal quote into the raw-string delimiter
    // instead of the pattern content, silently dropping it.
    static ref RE_MEASUREMENT: Regex =
        Regex::new(r##"(\W|^)([-0-9.]+\s*)'(\s*[-0-9.]+)""##).unwrap();
    static ref RE_NESTED_OPEN_DOUBLE: FancyRegex = FancyRegex::new(r#"(?<=\W)"(?=\w)"#).unwrap();
    static ref RE_NESTED_OPEN_SINGLE: FancyRegex = FancyRegex::new(r"(?<=\W)'(?=\w)").unwrap();
    static ref RE_NESTED_CLOSE_DOUBLE: FancyRegex = FancyRegex::new(r#"(?<=\w)"(?=\W)"#).unwrap();
    static ref RE_NESTED_CLOSE_SINGLE: FancyRegex = FancyRegex::new(r"(?<=\w)'(?=\W)").unwrap();

    // Punctuation class
    static ref PUNCT_CLASS: &'static str = r##"[!"#\$\%'()*+,-.\/:;<=>?\@\[\\\]\^_`{|}~]"##;
}

pub fn smarty_pants(text: &str, attr: &str) -> String {
    if attr == "0" {
        return text.to_string();
    }

    let (do_dashes, do_backticks, do_quotes, do_ellipses, do_stupefy) = parse_attr(attr);

    // We iterate tokens similar to Python: (tag, text)
    // Python _tokenize splits by tags.
    // Simplifying: Rust regex find_iter could work if we construct a tokenizer.
    // For now, let's implement the core transforms on the whole string if safe,
    // but Calibre carefully skips tags.

    // Simple tokenizer: split by <...>
    // Regex for tags: <[^>]*> matches widely but <...> might contain > in quotes.
    // Python SmartyPants uses:
    // _tokenize implementation (not fully shown in snippet but usually regex split).
    // Let's assume we can split by tags.

    // Basic implementation:
    // Just split by simple tag regex.
    let tag_regex = Regex::new(r"(<!--.*?-->|<[^>]*>)").unwrap();

    let mut result = String::new();
    let mut last_char: Option<char> = None;
    let mut in_pre = false;
    let mut skipped_tag_stack: Vec<String> = Vec::new();
    let mut last_end = 0;

    // ...
    // Note: In Python, skipped tags also update prev_token_last_char?
    // "prev_token_last_char = last_char" happens at end of loop.
    // "last_char = t[-1:]".
    // So even tags update context.

    for mat in tag_regex.find_iter(text) {
        let start = mat.start();
        let end = mat.end();

        // Text before tag
        if start > last_end {
            let t = &text[last_end..start];
            let processed = process_token(
                t,
                do_dashes,
                do_backticks,
                do_quotes,
                do_ellipses,
                do_stupefy,
                in_pre,
                last_char,
            );
            result.push_str(&processed);

            if let Some(c) = t.chars().last() {
                last_char = Some(c);
            }
        }

        // The tag itself
        let tag = mat.as_str();
        result.push_str(tag);

        // Update last_char from tag
        if let Some(c) = tag.chars().last() {
            last_char = Some(c);
        }

        // Handle skip tags
        if let Some(cap) = TAGS_TO_SKIP_REGEX.captures(tag) {
            let is_close = !cap.get(1).map_or(true, |m| m.as_str().is_empty()); // "/" group 1
            let tag_name = cap.get(2).unwrap().as_str().to_lowercase();
            let is_self_closing = SELF_CLOSING_REGEX.is_match(tag);

            if !is_self_closing {
                if !is_close {
                    // open
                    skipped_tag_stack.push(tag_name);
                    in_pre = true;
                } else {
                    // close
                    if let Some(last) = skipped_tag_stack.last() {
                        if *last == tag_name {
                            skipped_tag_stack.pop();
                        }
                    }
                    if skipped_tag_stack.is_empty() {
                        in_pre = false;
                    }
                }
            }
        }

        last_end = end;
    }

    // Remaining text
    if last_end < text.len() {
        let t = &text[last_end..];
        let processed = process_token(
            t,
            do_dashes,
            do_backticks,
            do_quotes,
            do_ellipses,
            do_stupefy,
            in_pre,
            last_char,
        );
        result.push_str(&processed);
    }

    result
}

fn process_token(
    text: &str,
    do_dashes: i32,
    do_backticks: i32,
    do_quotes: i32,
    do_ellipses: i32,
    do_stupefy: i32,
    in_pre: bool,
    prev_last_char: Option<char>,
) -> String {
    if in_pre {
        return text.to_string();
    }

    let mut t = text.to_string();
    t = process_escapes(&t);
    t = t.replace("&quot;", "\"");

    if do_dashes == 1 {
        t = educate_dashes(&t);
    } else if do_dashes == 2 {
        t = educate_dashes_old_school(&t);
    } else if do_dashes == 3 {
        t = educate_dashes_old_school_inverted(&t);
    }

    if do_ellipses == 1 {
        t = educate_ellipses(&t);
    }

    if do_backticks == 1 {
        t = educate_backticks(&t);
    } else if do_backticks == 2 {
        t = educate_single_backticks(&educate_backticks(&t));
    }

    if do_quotes != 0 {
        if t == "'" {
            // Special case: single-character ' token
            // if re.match(r'\S', prev_token_last_char):
            let is_whitespace = prev_last_char.map_or(true, |c| c.is_whitespace());
            if !is_whitespace {
                t = "&#8217;".to_string();
            } else {
                t = "&#8216;".to_string();
            }
        } else if t == "\"" {
            // Special case: single-character " token
            let is_whitespace = prev_last_char.map_or(true, |c| c.is_whitespace());
            if !is_whitespace {
                t = "&#8221;".to_string();
            } else {
                t = "&#8220;".to_string();
            }
        } else {
            t = educate_quotes(&t);
        }
    }

    if do_stupefy == 1 {
        t = stupefy_entities(&t);
    }

    t
}

fn parse_attr(attr: &str) -> (i32, i32, i32, i32, i32) {
    let mut do_dashes = 0;
    let mut do_backticks = 0;
    let mut do_quotes = 0;
    let mut do_ellipses = 0;
    let mut do_stupefy = 0;

    if attr == "1" {
        do_quotes = 1;
        do_backticks = 1;
        do_dashes = 1;
        do_ellipses = 1;
    } else if attr == "2" {
        do_quotes = 1;
        do_backticks = 1;
        do_dashes = 2;
        do_ellipses = 1;
    } else if attr == "3" {
        do_quotes = 1;
        do_backticks = 1;
        do_dashes = 3;
        do_ellipses = 1;
    } else if attr == "-1" {
        do_stupefy = 1;
    } else {
        for c in attr.chars() {
            match c {
                'q' => do_quotes = 1,
                'b' => do_backticks = 1,
                'B' => do_backticks = 2,
                'd' => do_dashes = 1,
                'D' => do_dashes = 2,
                'i' => do_dashes = 3,
                'e' => do_ellipses = 1,
                _ => {}
            }
        }
    }
    (do_dashes, do_backticks, do_quotes, do_ellipses, do_stupefy)
}

fn educate_dashes(text: &str) -> String {
    text.replace("---", "&#8211;").replace("--", "&#8212;")
}

fn educate_dashes_old_school(text: &str) -> String {
    text.replace("---", "&#8212;").replace("--", "&#8211;")
}

fn educate_dashes_old_school_inverted(text: &str) -> String {
    text.replace("---", "&#8211;").replace("--", "&#8212;")
}

fn educate_ellipses(text: &str) -> String {
    text.replace("...", "&#8230;").replace(". . .", "&#8230;")
}

fn educate_backticks(text: &str) -> String {
    text.replace("``", "&#8220;").replace("''", "&#8221;")
}

fn educate_single_backticks(text: &str) -> String {
    text.replace("`", "&#8216;").replace("'", "&#8217;")
}

/// Port of `educateQuotes`.
fn educate_quotes(text: &str) -> String {
    let mut text = text.to_string();

    // Special case if the very first character is a quote followed by
    // punctuation at a non-word-break. Close the quotes by brute force.
    text = RE_LEADING_SINGLE_QUOTE
        .replace_all(&text, "&#8217;")
        .into_owned();
    text = RE_LEADING_DOUBLE_QUOTE
        .replace_all(&text, "&#8221;")
        .into_owned();

    // Special case for double sets of quotes, e.g.:
    //   He said, "'Quoted' words in a larger quote."
    text = RE_DOUBLE_SINGLE_OPEN
        .replace_all(&text, "&#8220;&#8216;")
        .into_owned();
    text = RE_SINGLE_DOUBLE_OPEN
        .replace_all(&text, "&#8216;&#8220;")
        .into_owned();
    text = RE_DOUBLE_DOUBLE_OPEN
        .replace_all(&text, "&#8220;&#8220;")
        .into_owned();
    text = RE_SINGLE_SINGLE_OPEN
        .replace_all(&text, "&#8216;&#8216;")
        .into_owned();

    // Four independent, sequential literal substring replacements
    // (Python's `text.replace(...)`, not regex).
    text = text.replace("\"'", "&#8221;&#8217;");
    text = text.replace("'\"", "&#8217;&#8221;");
    text = text.replace("\"\"", "&#8221;&#8221;");
    text = text.replace("''", "&#8217;&#8217;");

    // Special case for decade abbreviations (the '80s -> '80s):
    text = RE_DECADE.replace_all(&text, "$1&#8217;").into_owned();
    // Measurements in feet and inches or longitude/latitude:
    // 19' 43.5" -> 19′ 43.5″
    text = RE_MEASUREMENT
        .replace_all(&text, "$1$2&#8242;$3&#8243;")
        .into_owned();

    // Special case for quotes nested inside other entities, e.g.:
    //   A double quote--"within dashes"--would be nice.
    text = RE_NESTED_OPEN_DOUBLE
        .replace_all(&text, "&#8220;")
        .into_owned();
    text = RE_NESTED_OPEN_SINGLE
        .replace_all(&text, "&#8216;")
        .into_owned();
    text = RE_NESTED_CLOSE_DOUBLE
        .replace_all(&text, "&#8221;")
        .into_owned();
    text = RE_NESTED_CLOSE_SINGLE
        .replace_all(&text, "&#8217;")
        .into_owned();

    // Get most opening single quotes:
    text = OPENING_SINGLE_QUOTES_REGEX
        .replace_all(&text, "$1&#8216;")
        .into_owned();

    // Closing single quotes:
    text = CLOSING_SINGLE_QUOTES_REGEX1
        .replace_all(&text, "$1&#8217;")
        .into_owned();
    text = CLOSING_SINGLE_QUOTES_REGEX2
        .replace_all(&text, "$1&#8217;$2")
        .into_owned();

    // Any remaining single quotes should be opening ones:
    text = text.replace('\'', "&#8216;");

    // Get most opening double quotes:
    text = OPENING_DOUBLE_QUOTES_REGEX
        .replace_all(&text, "$1&#8220;")
        .into_owned();

    // Double closing quotes:
    text = CLOSING_DOUBLE_QUOTES_REGEX1
        .replace_all(&text, "&#8221;")
        .into_owned();
    text = CLOSING_DOUBLE_QUOTES_REGEX2
        .replace_all(&text, "$1&#8221;")
        .into_owned();

    // A string that ends with -" is sometimes used for dialogue.
    if text.ends_with("-\"") {
        text.pop();
        text.push_str("&#8221;");
    }

    // Any remaining quotes should be opening ones.
    text = text.replace('"', "&#8220;");

    text
}

fn stupefy_entities(text: &str) -> String {
    text.replace("&#8211;", "-")
        .replace("&#8212;", "--")
        .replace("&#8216;", "'")
        .replace("&#8217;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&#8230;", "...")
}

fn process_escapes(text: &str) -> String {
    text.replace(r"\\", "&#92;")
        .replace(r#"\""#, "&#34;")
        .replace(r"\'", "&#39;")
        .replace(r"\.", "&#46;")
        .replace(r"\-", "&#45;")
        .replace(r"\`", "&#96;")
}

#[cfg(test)]
mod tests {
    use super::*;

    // `educate_quotes` (called from `smarty_pants(text, "q")`) used to
    // panic unconditionally on any non-trivial input: several of its
    // patterns need regex lookaround, which the plain `regex` crate
    // (used here at the time) can't compile at all -- `Regex::new(...)`
    // would fail and the `.unwrap()` right after it would panic. This
    // module had no tests, so nothing caught it; it was found and
    // fixed while porting `calibre.ebooks.textile.functions` (issue
    // #53), which is the first real caller of `smarty_pants` in this
    // codebase. These cases are transcribed from a live run of
    // `old_src/src/calibre/utils/smartypants.py`'s `smartyPants(text,
    // 'q')`.

    #[test]
    fn single_quotes_become_curly() {
        assert_eq!(
            smarty_pants("'Quoted' words", "q"),
            "&#8216;Quoted&#8217; words"
        );
    }

    #[test]
    fn double_quotes_with_embedded_apostrophe() {
        assert_eq!(
            smarty_pants("\"Isn't this fun?\"", "q"),
            "&#8220;Isn&#8217;t this fun?&#8221;"
        );
    }

    #[test]
    fn decade_abbreviation() {
        assert_eq!(smarty_pants("the '80s", "q"), "the &#8217;80s");
    }

    #[test]
    fn feet_and_inches_measurement() {
        assert_eq!(
            smarty_pants("19' 43.5\" wide", "q"),
            "19&#8242; 43.5&#8243; wide"
        );
    }

    #[test]
    fn nested_double_then_single_quotes() {
        assert_eq!(
            smarty_pants("He said, \"'Quoted' words in a larger quote.\"", "q"),
            "He said, &#8220;&#8216;Quoted&#8217; words in a larger quote.&#8221;"
        );
    }

    #[test]
    fn quotes_nested_inside_dashes() {
        assert_eq!(
            smarty_pants("A double quote--\"within dashes\"--would be nice.", "q"),
            "A double quote--&#8220;within dashes&#8221;--would be nice."
        );
    }

    #[test]
    fn attr_zero_is_a_no_op() {
        assert_eq!(
            smarty_pants("don't \"quote\" me", "0"),
            "don't \"quote\" me"
        );
    }
}
