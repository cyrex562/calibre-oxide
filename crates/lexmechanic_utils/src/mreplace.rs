use aho_corasick::AhoCorasick;
use std::collections::HashMap;

pub struct MReplace {
    ac: AhoCorasick,
    replacements: Vec<String>,
}

impl MReplace {
    pub fn new(map: &HashMap<&str, &str>) -> Self {
        let mut patterns = Vec::new();
        let mut replacements = Vec::new();
        
        // Ensure deterministic order for AC construction (though AC handles overlap by longest match or order?)
        // AhoCorasick default matches longest unless configured otherwise?
        // Python MReplace sorts keys by length (reverse=True). 
        // This implies longest-match wins in Python regex alternation (a|b|c).
        // AhoCorasick has MatchKind::LeftmostLongest. We should use that.
        // But AhoCorasick::new() usually does Standard.
        // We should configure it.
        
        // Sorting keys by length desc for consistent "patterns" vector
        let mut keys: Vec<&str> = map.keys().cloned().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len())); // Descending length
        
        for k in &keys {
            patterns.push(*k);
            replacements.push(map.get(k).unwrap().to_string());
        }
        
        let ac = AhoCorasick::builder()
            .match_kind(aho_corasick::MatchKind::LeftmostLongest) 
            .build(&patterns)
            .unwrap();
            
        MReplace { ac, replacements }
    }
    
    pub fn replace(&self, text: &str) -> String {
        self.ac.replace_all(text, &self.replacements)
    }
}
