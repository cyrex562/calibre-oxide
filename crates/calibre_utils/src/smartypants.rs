use regex::Regex;
use lazy_static::lazy_static;

// Helper to compile regexes
lazy_static! {
    static ref TAGS_TO_SKIP_REGEX: Regex = Regex::new(r"(?i)<(/?)(style|pre|code|kbd|script|math)[^>]*>").unwrap();
    static ref SELF_CLOSING_REGEX: Regex = Regex::new(r"/\s*>$").unwrap();
    
    // Quotes regexes
    static ref OPENING_SINGLE_QUOTES_REGEX: Regex = Regex::new(r"(?x)
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
    
    static ref CLOSING_SINGLE_QUOTES_REGEX1: Regex = Regex::new(r"(?x)
            ([^\ \t\r\n\[\{\(\-])
            '
            (?!\s | s\b | \d)
            ").unwrap();

    static ref CLOSING_SINGLE_QUOTES_REGEX2: Regex = Regex::new(r"(?x)
            ([^\ \t\r\n\[\{\(\-])
            '
            (\s | s\b)
            ").unwrap();

    static ref OPENING_DOUBLE_QUOTES_REGEX: Regex = Regex::new(r"(?x)
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
    
    static ref CLOSING_DOUBLE_QUOTES_REGEX1: Regex = Regex::new(r#"(?x)
            "
            (?=\s)
            "#).unwrap();
            
    static ref CLOSING_DOUBLE_QUOTES_REGEX2: Regex = Regex::new(r#"(?x)
            ([^\ \t\r\n\[\{\(\-])   # character that indicates the quote should be closing
            "
            "#).unwrap();
            
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
            let processed = process_token(t, do_dashes, do_backticks, do_quotes, do_ellipses, do_stupefy, in_pre, last_char);
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
        let processed = process_token(t, do_dashes, do_backticks, do_quotes, do_ellipses, do_stupefy, in_pre, last_char);
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
    prev_last_char: Option<char>
) -> String {
    if in_pre {
        return text.to_string();
    }
    
    let mut t = text.to_string();
    t = process_escapes(&t);
    t = t.replace("&quot;", "\"");
    
    if do_dashes == 1 { t = educate_dashes(&t); }
    else if do_dashes == 2 { t = educate_dashes_old_school(&t); }
    else if do_dashes == 3 { t = educate_dashes_old_school_inverted(&t); }
    
    if do_ellipses == 1 { t = educate_ellipses(&t); }
    
    if do_backticks == 1 { t = educate_backticks(&t); }
    else if do_backticks == 2 { t = educate_single_backticks(&educate_backticks(&t)); }
    
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
        do_quotes = 1; do_backticks = 1; do_dashes = 1; do_ellipses = 1;
    } else if attr == "2" {
        do_quotes = 1; do_backticks = 1; do_dashes = 2; do_ellipses = 1;
    } else if attr == "3" {
        do_quotes = 1; do_backticks = 1; do_dashes = 3; do_ellipses = 1;
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

fn educate_quotes(text: &str) -> String {
    let mut text = text.to_string();
    let punct = *PUNCT_CLASS;
    
    // brute force
    // ^'(?={punct}\\B)
    let re = Regex::new(&format!(r"(?m)^'(?={}\B)", punct)).unwrap();
    text = re.replace_all(&text, "&#8217;").to_string();
    
    let re = Regex::new(&format!(r#"(?m)^"(?={}\B)"#, punct)).unwrap(); // ^"
    text = re.replace_all(&text, "&#8221;").to_string();
    
    // Double sets
    text = Regex::new(r#""'(?=\w)"#).unwrap().replace_all(&text, "&#8220;&#8216;").to_string();
    text = Regex::new(r#"' "(?=\w)"#).unwrap().replace_all(&text, "&#8216;&#8220;").to_string(); // Adjusted regex: ' " vs '"
    // Python: r'''"'(?=\w)''', r''''"(?=\w)''' (Wait, Python regex is r''' ' " (?=\w) '''?)
    // Python source: r''''"(?=\w)''' -> ' " (single then double).
    text = Regex::new(r#"' "(?=\w)"#).unwrap().replace_all(&text, "&#8216;&#8220;").to_string();
    
    text = text.replace("\"'", "&#8221;&#8217;");
    text = text.replace("'\"", "&#8217;&#8221;");
    text = text.replace("\"\"", "&#8221;&#8221;");
    text = text.replace("''", "&#8217;&#8217;");
    
    // Decades
    text = Regex::new(r"(\W|^)'(?=\d{2}s)").unwrap().replace_all(&text, "$1&#8217;").to_string();
    
    // Measurements
    text = Regex::new(r#"(\W|^)([-0-9.]+\s*)'(\s*[-0-9.]+)"#).unwrap().replace_all(&text, r"$1$2&#8242;$3&#8243;").to_string();
    
    // Nested
    text = Regex::new(r#"(?<=\W)"(?=\w)"#).unwrap().replace_all(&text, "&#8220;").to_string();
    text = Regex::new(r#"(?<=\W)'(?=\w)"#).unwrap().replace_all(&text, "&#8216;").to_string();
    text = Regex::new(r#"(?<=\w)"(?=\W)"#).unwrap().replace_all(&text, "&#8221;").to_string();
    text = Regex::new(r#"(?<=\w)'(?=\W)"#).unwrap().replace_all(&text, "&#8217;").to_string();
    
    // Opening single
    text = OPENING_SINGLE_QUOTES_REGEX.replace_all(&text, "$1&#8216;").to_string();
    
    // Closing single
    text = CLOSING_SINGLE_QUOTES_REGEX1.replace_all(&text, "$1&#8217;").to_string();
    text = CLOSING_SINGLE_QUOTES_REGEX2.replace_all(&text, "$1&#8217;$2").to_string();
    
    // Remaining single
    text = text.replace("'", "&#8216;");
    
    // Opening double
    text = OPENING_DOUBLE_QUOTES_REGEX.replace_all(&text, "$1&#8220;").to_string();
    
    // Closing double
    text = CLOSING_DOUBLE_QUOTES_REGEX1.replace_all(&text, "&#8221;").to_string();
    text = CLOSING_DOUBLE_QUOTES_REGEX2.replace_all(&text, "$1&#8221;").to_string();
    
    // Finish -"
    if text.ends_with("-\"") {
        text.pop(); // "
        text.push_str("&#8221;");
    }
    
    // Remaining double
    text = text.replace("\"", "&#8220;");
    
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
