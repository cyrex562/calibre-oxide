use anyhow::{Context, Result};
use regex::Regex;

pub struct SearchReplace {
    rules: Vec<(Regex, String)>,
}

impl SearchReplace {
    pub fn new() -> Self {
        SearchReplace { rules: Vec::new() }
    }

    pub fn add_rule(&mut self, pattern: &str, replacement: &str) -> Result<()> {
        let re = Regex::new(pattern).context("Invalid regex pattern")?;
        self.rules.push((re, replacement.to_string()));
        Ok(())
    }

    pub fn process(&self, content: &str) -> String {
        let mut result = content.to_string();
        for (re, replacement) in &self.rules {
            result = re.replace_all(&result, replacement.as_str()).to_string();
        }
        result
    }
}
