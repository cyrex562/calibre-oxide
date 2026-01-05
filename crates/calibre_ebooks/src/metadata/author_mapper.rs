use regex::Regex;
use std::collections::{HashSet, VecDeque};

/// Capitalize an author name token with special handling for particles, initials, and Scottish names
pub fn cap_author_token(token: &str) -> String {
    let lt = token.to_lowercase();

    // Handle particles that should stay lowercase
    if ["von", "de", "el", "van", "le"].contains(&lt.as_str()) {
        return lt;
    }

    // Handle initials like "J.K." -> "J. K."
    if Regex::new(r"^([^\d\W]\.){2,}$").unwrap().is_match(&lt) {
        let parts: Vec<&str> = token.split('.').collect();
        return parts
            .iter()
            .map(|p| capitalize_first(p))
            .collect::<Vec<_>>()
            .join(". ")
            .trim()
            .to_string();
    }

    // Handle Scottish names (Mc/Mac prefix)
    let scots_name = if lt.starts_with("mc") && lt.len() > 2 {
        Some(2)
    } else if lt.starts_with("mac") && lt.len() > 3 {
        Some(3)
    } else {
        None
    };

    let mut ans = capitalize_first(token);

    // Capitalize after Mc/Mac
    if let Some(scots_len) = scots_name {
        if ans.len() > scots_len {
            let (prefix, rest) = ans.split_at(scots_len);
            if let Some(first_char) = rest.chars().next() {
                ans = format!(
                    "{}{}{}",
                    prefix,
                    first_char.to_uppercase(),
                    &rest[first_char.len_utf8()..]
                );
            }
        }
    }

    // Capitalize after hyphen or apostrophe
    for separator in &['-', '\''] {
        if let Some(idx) = ans.find(*separator) {
            if ans.len() > idx + 1 {
                let (before, after) = ans.split_at(idx + 1);
                if let Some(first_char) = after.chars().next() {
                    ans = format!(
                        "{}{}{}",
                        before,
                        first_char.to_uppercase(),
                        &after[first_char.len_utf8()..]
                    );
                }
            }
        }
    }

    ans
}

/// Helper function to capitalize the first character
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Helper function for case-insensitive lowercase (simplified ICU)
fn icu_lower(s: &str) -> String {
    s.to_lowercase()
}

/// Rule structure for author mapping
#[derive(Debug, Clone)]
pub struct Rule {
    pub action: String,
    pub query: String,
    pub replace: Option<String>,
    pub match_type: String,
}

/// Create a matcher function based on rule type
pub fn matcher(rule: &Rule) -> Box<dyn Fn(&str) -> bool> {
    match rule.match_type.as_str() {
        "one_of" => {
            let authors: HashSet<String> =
                rule.query.split('&').map(|s| icu_lower(s.trim())).collect();
            Box::new(move |x: &str| authors.contains(&icu_lower(x)))
        }
        "not_one_of" => {
            let authors: HashSet<String> =
                rule.query.split('&').map(|s| icu_lower(s.trim())).collect();
            Box::new(move |x: &str| !authors.contains(&icu_lower(x)))
        }
        "matches" => {
            let pat = Regex::new(&rule.query).unwrap_or_else(|_| Regex::new("^$").unwrap());
            Box::new(move |x: &str| pat.is_match(x))
        }
        "not_matches" => {
            let pat = Regex::new(&rule.query).unwrap_or_else(|_| Regex::new("^$").unwrap());
            Box::new(move |x: &str| !pat.is_match(x))
        }
        "has" => {
            let s = icu_lower(&rule.query);
            Box::new(move |x: &str| icu_lower(x).contains(&s))
        }
        _ => Box::new(|_: &str| false),
    }
}

/// Apply transformation rules to an author name
pub fn apply_rules(author: &str, rules: &[(Rule, Box<dyn Fn(&str) -> bool>)]) -> Vec<String> {
    let mut ans = Vec::new();
    let mut authors = VecDeque::new();
    authors.push_back(author.to_string());

    let mut maxiter = 20;

    while let Some(current_author) = authors.pop_front() {
        if maxiter == 0 {
            ans.push(current_author);
            continue;
        }
        maxiter -= 1;

        let lauthor = icu_lower(&current_author);
        let mut matched = false;

        for (rule, matches_fn) in rules {
            if matches_fn(&lauthor) {
                matched = true;
                match rule.action.as_str() {
                    "replace" => {
                        let replacement = rule.replace.as_ref().unwrap_or(&rule.query);
                        let new_author = if rule.match_type.contains("matches") {
                            // Regex replacement
                            let pat = Regex::new(&rule.query).unwrap();
                            pat.replace(&current_author, replacement.as_str())
                                .to_string()
                        } else {
                            replacement.clone()
                        };

                        // Handle multi-author replacement (& separator)
                        if new_author.contains('&') {
                            let mut replacement_authors = Vec::new();
                            let mut self_added = false;

                            for rauthor in new_author.split('&').map(|s| s.trim()) {
                                if icu_lower(rauthor) == lauthor {
                                    if !self_added {
                                        ans.push(rauthor.to_string());
                                        self_added = true;
                                    }
                                } else {
                                    replacement_authors.push(rauthor.to_string());
                                }
                            }

                            // Add replacement authors to front of queue
                            for ra in replacement_authors.into_iter().rev() {
                                authors.push_front(ra);
                            }
                        } else if icu_lower(&new_author) == lauthor {
                            // Case change or self replacement
                            ans.push(new_author);
                        } else {
                            // Different author, add to front of queue
                            authors.push_front(new_author);
                        }
                    }
                    "capitalize" => {
                        let capitalized = current_author
                            .split_whitespace()
                            .map(|token| cap_author_token(token))
                            .collect::<Vec<_>>()
                            .join(" ");
                        ans.push(capitalized);
                    }
                    "lower" => {
                        ans.push(icu_lower(&current_author));
                    }
                    "upper" => {
                        ans.push(current_author.to_uppercase());
                    }
                    _ => {
                        ans.push(current_author.clone());
                    }
                }
                break;
            }
        }

        if !matched {
            // No rule matched, keep as is
            ans.push(current_author);
        }
    }

    // Add any remaining authors from queue
    ans.extend(authors);
    ans
}

/// Remove duplicates while preserving order (case-insensitive)
pub fn uniq(vals: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    vals.iter()
        .filter(|v| {
            let key = icu_lower(v);
            if seen.contains(&key) {
                false
            } else {
                seen.insert(key);
                true
            }
        })
        .cloned()
        .collect()
}

/// Compile rules into (rule, matcher) pairs
pub fn compile_rules(rules: &[Rule]) -> Vec<(Rule, Box<dyn Fn(&str) -> bool>)> {
    rules.iter().map(|r| (r.clone(), matcher(r))).collect()
}

/// Map authors through transformation rules
pub fn map_authors(authors: &[String], rules: &[(Rule, Box<dyn Fn(&str) -> bool>)]) -> Vec<String> {
    if authors.is_empty() {
        return Vec::new();
    }

    if rules.is_empty() {
        return authors.to_vec();
    }

    let mut ans = Vec::new();
    for author in authors {
        ans.extend(apply_rules(author, rules));
    }

    // Filter out empty strings and remove duplicates
    uniq(
        &ans.into_iter()
            .filter(|a| !a.is_empty())
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: &str, query: &str, replace: Option<&str>, match_type: &str) -> Rule {
        Rule {
            action: action.to_string(),
            query: query.to_string(),
            replace: replace.map(|s| s.to_string()),
            match_type: match_type.to_string(),
        }
    }

    fn run(rules: Vec<Rule>, authors: &str, expected: &str) {
        let author_list: Vec<String> = authors.split('&').map(|s| s.trim().to_string()).collect();
        let expected_list: Vec<String> =
            expected.split('&').map(|s| s.trim().to_string()).collect();
        let compiled = compile_rules(&rules);
        let result = map_authors(&author_list, &compiled);
        assert_eq!(result, expected_list, "Failed for authors: {}", authors);
    }

    #[test]
    fn test_capitalize() {
        run(
            vec![rule("capitalize", "t1&t2", None, "one_of")],
            "t1&x1",
            "T1&x1",
        );
    }

    #[test]
    fn test_upper() {
        run(
            vec![rule("upper", "ta&t2", None, "one_of")],
            "ta&x1",
            "TA&x1",
        );
    }

    #[test]
    fn test_lower() {
        run(
            vec![rule("lower", "ta&x1", None, "one_of")],
            "TA&X1",
            "ta&x1",
        );
    }

    #[test]
    fn test_replace() {
        run(
            vec![rule("replace", "t1", Some("t2"), "one_of")],
            "t1&x1",
            "t2&x1",
        );
    }

    #[test]
    fn test_regex_replace() {
        run(
            vec![rule("replace", "(.)1", Some("${1}2"), "matches")],
            "t1&x1",
            "t2&x2",
        );
    }

    #[test]
    fn test_multi_author_replace() {
        run(
            vec![rule("replace", "t1", Some("t2 & t3"), "one_of")],
            "t1&x1",
            "t2&t3&x1",
        );
    }

    #[test]
    fn test_self_replacement() {
        run(
            vec![rule("replace", "t1", Some("t1"), "one_of")],
            "t1&x1",
            "t1&x1",
        );
    }

    #[test]
    fn test_chained_rules() {
        run(
            vec![
                rule("replace", "t1", Some("t2"), "one_of"),
                rule("replace", "t2", Some("t1"), "one_of"),
            ],
            "t1&t2",
            "t1&t2",
        );
    }

    #[test]
    fn test_case_change() {
        run(
            vec![rule("replace", "a", Some("A"), "one_of")],
            "a&b",
            "A&b",
        );
    }

    #[test]
    fn test_has_matcher() {
        run(vec![rule("replace", "L", Some("T"), "has")], "L", "T");
    }

    #[test]
    fn test_cap_author_token() {
        assert_eq!(cap_author_token("john"), "John");
        assert_eq!(cap_author_token("von"), "von");
        assert_eq!(cap_author_token("de"), "de");
        assert_eq!(cap_author_token("mcdonald"), "McDonald");
        assert_eq!(cap_author_token("macdonald"), "MacDonald");
        assert_eq!(cap_author_token("o'brien"), "O'Brien");
        assert_eq!(cap_author_token("jean-paul"), "Jean-Paul");
    }

    #[test]
    fn test_uniq() {
        let vals = vec![
            "John".to_string(),
            "jane".to_string(),
            "JOHN".to_string(),
            "Jane".to_string(),
            "Bob".to_string(),
        ];
        let result = uniq(&vals);
        assert_eq!(result, vec!["John", "jane", "Bob"]);
    }
}
