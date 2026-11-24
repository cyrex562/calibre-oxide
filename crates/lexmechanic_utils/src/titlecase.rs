use regex::Regex;
use lazy_static::lazy_static;
use crate::icu::{lower, upper, capitalize};

lazy_static! {
    static ref SMALL_WORDS_REGEX: Regex = Regex::new(r"(?i)^(a|an|and|as|at|but|by|en|for|if|in|of|on|or|the|to|v\.?|via|vs\.?)$").unwrap();
    static ref INLINE_PERIOD_REGEX: Regex = Regex::new(r"(?i)[a-z][.][a-z]").unwrap();
    static ref UC_ELSEWHERE_REGEX: Regex = Regex::new(r##"(?x)
        [!"#\$\%'()*+,-.\/:;<=>?\@\[\\\]\^_`{|}~]*?
        [a-zA-Z]+[A-Z]+?
    "##).unwrap();
    // CAPFIRST depends on unicode word chars.
    static ref CAPFIRST_REGEX: Regex = Regex::new(r##"(?u)^[!"#\$\%'()*+,-.\/:;<=>?\@\[\\\]\^_`{|}~]*?(\w)"##).unwrap();
    
    // SMALL_FIRST: punctuation followed by small word at start
    static ref SMALL_FIRST_REGEX: Regex = Regex::new(r"(?i)(?u)^([!#\$\%'()*+,-./:;<=>?@\[\\\]^_`{|}~]*)(a|an|and|as|at|but|by|en|for|if|in|of|on|or|the|to|v\.?|via|vs\.?)\b").unwrap();  
    // Removed " and & from punct here to match Python r'''...'''? Python used PUNCT which has everything.
    // Wait, regex crate doesn't allow set subtraction easily inside character classes unless strict.
    // I used literal string for PUNCT in Python: `!"#$%&'‘’()*+,\-‒–—―./:;?@[\\\]_`{|}~`
    // I'll stick to standard ASCII punct for now or verify against `smartypants` punct class.
    
    static ref SMALL_LAST_REGEX: Regex = Regex::new(r"(?i)(?u)\b(a|an|and|as|at|but|by|en|for|if|in|of|on|or|the|to|v\.?|via|vs\.?)[!#\$\%'()*+,-./:;<=>?@\[\\\]^_`{|}~]?$").unwrap();
    
    static ref SMALL_AFTER_NUM_REGEX: Regex = Regex::new(r"(?i)(?u)(\d+\s+)(a|an|the)\b").unwrap();
    
    static ref SUBPHRASE_REGEX: Regex = Regex::new(r"(?i)([:.;?!][ ])(a|an|and|as|at|but|by|en|for|if|in|of|on|or|the|to|v\.?|via|vs\.?)").unwrap();
    
    static ref APOS_SECOND_REGEX: Regex = Regex::new(r"(?i)^[dol]{1}['‘]{1}[a-z]+$").unwrap();
    static ref UC_INITIALS_REGEX: Regex = Regex::new(r"^(?:[A-Z]{1}\.{1}|[A-Z]{1}\.{1}[A-Z]{1})+$").unwrap();
}

pub fn titlecase(text: &str) -> String {
    let all_caps = upper(text) == text;
    
    // Python uses split(r'(\s+)') which keeps delimiters!
    // Simply split_whitespace() loses delimiters.
    // We need to preserve whitespace.
    // Use regex split?
    
    let start_regex = Regex::new(r"(\s+)").unwrap();
    // Rust regex split doesn't keep delimiters.
    // We must manually iterate matches.
    
    // Reconstructing Python's iterating:
    let mut line = Vec::new();
    let mut last = 0;
    for mat in start_regex.find_iter(text) {
        if mat.start() > last {
            line.push(text[last..mat.start()].to_string());
        }
        line.push(mat.as_str().to_string());
        last = mat.end();
    }
    if last < text.len() {
        line.push(text[last..].to_string());
    }
    // If text starts with whitespace, find_iter finds it first.
    
    let mut processed_line = Vec::new();
    for word in line {
        if word.trim().is_empty() {
            processed_line.push(word);
            continue;
        }
        
        // It's a word
        let mut w = word.to_string();
        
        if all_caps {
            if UC_INITIALS_REGEX.is_match(&w) {
                processed_line.push(w);
                continue;
            } else {
                w = lower(&w);
            }
        }
        
        if APOS_SECOND_REGEX.is_match(&w) {
            // d'Artagnan -> D'Artagnan
             // w[0] upper, w[2] upper
             // Rust indexing is byte based. Need chars.
             let chars: Vec<char> = w.chars().collect();
             if chars.len() > 2 {
                 let mut new_w = String::new();
                 new_w.push(chars[0].to_uppercase().next().unwrap());
                 new_w.push(chars[1]);
                 new_w.push(chars[2].to_uppercase().next().unwrap());
                 for c in &chars[3..] { new_w.push(*c); }
                 processed_line.push(new_w);
                 continue;
             }
        }
        
        if INLINE_PERIOD_REGEX.is_match(&w) || UC_ELSEWHERE_REGEX.is_match(&w) {
            processed_line.push(w);
            continue;
        }
        
        if SMALL_WORDS_REGEX.is_match(&w) {
            processed_line.push(lower(&w));
            continue;
        }
        
        // Hyphenated
        let parts: Vec<&str> = w.split('-').collect(); // Split loses separators if multiple?
        // simple split matches Python's split('-')
        let mut hyphens = Vec::new();
        for p in parts.iter() {
             // capfirst
             hyphens.push(CAPFIRST_REGEX.replace(p, |caps: &regex::Captures| {
                 upper(&caps[1])
             }).to_string());
        }
        processed_line.push(hyphens.join("-"));
    }
    
    let mut result = processed_line.join("");
    
    // Fixups
    result = SMALL_FIRST_REGEX.replace_all(&result, |caps: &regex::Captures| {
        format!("{}{}", &caps[1], capitalize(&caps[2]))
    }).to_string();
    
    result = SMALL_AFTER_NUM_REGEX.replace_all(&result, |caps: &regex::Captures| {
        format!("{}{}", &caps[1], capitalize(&caps[2]))
    }).to_string();
    
    result = SMALL_LAST_REGEX.replace_all(&result, |caps: &regex::Captures| {
        capitalize(&caps[0]) // caps[0] is the whole match? Python: capitalize(m.group(0))
    }).to_string();
    
    result = SUBPHRASE_REGEX.replace_all(&result, |caps: &regex::Captures| {
        format!("{}{}", &caps[1], capitalize(&caps[2]))
    }).to_string();
    
    result
}
