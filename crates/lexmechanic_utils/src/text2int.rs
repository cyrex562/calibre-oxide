use regex::Regex;
use lazy_static::lazy_static;
use std::collections::HashMap;

lazy_static! {
    static ref NUMWORDS: HashMap<&'static str, (u64, u64)> = {
        let mut m = HashMap::new();
        let units = ["zero", "one", "two", "three", "four", "five", "six",
                     "seven", "eight", "nine", "ten", "eleven", "twelve",
                     "thirteen", "fourteen", "fifteen", "sixteen", "seventeen",
                     "eighteen", "nineteen"];
        let tens = ["", "", "twenty", "thirty", "forty", "fifty", "sixty",
                    "seventy", "eighty", "ninety"];
        let scales = ["hundred", "thousand", "million", "billion", "trillion",
                      "quadrillion", "quintillion", "sexillion", "septillion",
                      "octillion", "nonillion", "decillion"];

        m.insert("and", (1, 0));
        for (idx, word) in units.iter().enumerate() {
            m.insert(word, (1, idx as u64));
        }
        for (idx, word) in tens.iter().enumerate() {
            if idx < 2 { continue; } // skip empty
            m.insert(word, (1, (idx as u64) * 10));
        }
        for (idx, word) in scales.iter().enumerate() {
            let power = if idx == 0 { 2 } else { idx as u32 * 3 };
            m.insert(word, (10u64.pow(power), 0));
        }
        m
    };

    static ref ORDINAL_WORDS: HashMap<&'static str, u64> = {
        let mut m = HashMap::new();
        m.insert("first", 1); m.insert("second", 2); m.insert("third", 3);
        m.insert("fifth", 5); m.insert("eighth", 8); m.insert("ninth", 9);
        m.insert("twelfth", 12);
        m
    };
    
    static ref SPLIT_REGEX: Regex = Regex::new(r"[\s-]+").unwrap();
}

pub fn text2int(text: &str) -> Option<u64> {
    let mut current = 0;
    let mut result = 0;
    
    let tokens: Vec<&str> = SPLIT_REGEX.split(text).collect();
    
    for token in tokens {
        if token.is_empty() { continue; }
        
        let mut word = token.to_lowercase();
        let scale; 
        let increment;

        if let Some(&val) = ORDINAL_WORDS.get(word.as_str()) {
            scale = 1;
            increment = val;
        } else {
            // Check ordinals endings
            let ordinal_endings = [("ieth", "y"), ("th", "")];
            for (ending, replacement) in ordinal_endings {
                if word.ends_with(ending) {
                    word = format!("{}{}", &word[..word.len()-ending.len()], replacement);
                    break; 
                }
            }
            
            if let Some(&(s, i)) = NUMWORDS.get(word.as_str()) {
                scale = s;
                increment = i;
            } else {
                return None; // Illegal word
            }
        }
        
        if scale > 1 {
            current = std::cmp::max(1, current);
        }
        
        current = current * scale + increment;
        if scale > 100 {
            result += current;
            current = 0;
        }
    }
    
    Some(result + current)
}
