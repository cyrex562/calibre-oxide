//! Port of `old_src/src/calibre/ebooks/rtf2xml/check_brackets.py`
//! (`CheckBrackets`).
//!
//! Validates that the `ob<nu<open-brack<NNNN` / `cb<nu<clos-brack<NNNN`
//! bracket-marker lines emitted by [`super::process_tokens`] nest
//! correctly: every close must match the most recently opened bracket's
//! 4-digit sequence number, and none may be left open at end of file.

/// Result of [`check_brackets`]. Mirrors the Python's `(bool, str)`
/// return tuple from `CheckBrackets.check_brackets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BracketReport {
    pub balanced: bool,
    pub message: String,
}

/// Port of `CheckBrackets.check_brackets` (+ `open_brack`/
/// `close_brack`), operating directly on the intermediate-format text
/// rather than re-opening a file (see [`super::process_tokens`]'s
/// module docs for the line format).
///
/// Each candidate line is matched by its first 16 characters against
/// the fixed marker prefixes `ob<nu<open-brack` / `cb<nu<clos-brack`
/// (exactly as the Python's `line[:16]` does), and the bracket's
/// 4-digit sequence number is read from the last 4 characters (the
/// Python's `line[-5:-1]` drops one extra trailing character because
/// its `line` still carries the `\n` that `str::lines()` here already
/// strips).
pub fn check_brackets(content: &str) -> BracketReport {
    let mut bracket_count: i64 = 0;
    let mut open_bracket_num: Vec<String> = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let line_count = idx + 1;
        let token_info: &str = if line.len() >= 16 { &line[..16] } else { line };

        if token_info == "ob<nu<open-brack" {
            if let Some(num) = last_four(line) {
                open_bracket_num.push(num.to_string());
                bracket_count += 1;
            }
        } else if token_info == "cb<nu<clos-brack" {
            let Some(num) = last_four(line) else {
                return BracketReport {
                    balanced: false,
                    message: format!("closed bracket doesn't match, line {line_count}"),
                };
            };
            match open_bracket_num.pop() {
                None => {
                    return BracketReport {
                        balanced: false,
                        message: format!("closed bracket doesn't match, line {line_count}"),
                    };
                }
                Some(last_num) if last_num != num => {
                    return BracketReport {
                        balanced: false,
                        message: format!("closed bracket doesn't match, line {line_count}"),
                    };
                }
                Some(_) => {
                    bracket_count -= 1;
                }
            }
        }
    }

    if bracket_count != 0 {
        return BracketReport {
            balanced: false,
            message: format!(
                "At end of file open and closed brackets don't match\ntotal number of brackets is {bracket_count}"
            ),
        };
    }
    BracketReport {
        balanced: true,
        message: "Brackets match!".to_string(),
    }
}

/// Port of `line[-5:-1]` (a Python line with its trailing `\n` still
/// attached) applied to a `str::lines()` line (trailing `\n` already
/// stripped): the last 4 characters.
fn last_four(line: &str) -> Option<&str> {
    let len = line.len();
    if len < 4 {
        return None;
    }
    Some(&line[len - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(n: u32) -> String {
        format!("ob<nu<open-brack<{n:04}\n")
    }

    fn cb(n: u32) -> String {
        format!("cb<nu<clos-brack<{n:04}\n")
    }

    #[test]
    fn empty_input_is_balanced() {
        let report = check_brackets("");
        assert!(report.balanced);
        assert_eq!(report.message, "Brackets match!");
    }

    #[test]
    fn simple_nested_pair_is_balanced() {
        let content = format!("{}{}{}", ob(1), ob(2), format!("{}{}", cb(2), cb(1)));
        let report = check_brackets(&content);
        assert!(report.balanced, "{}", report.message);
    }

    #[test]
    fn out_of_order_close_is_unbalanced() {
        // opens 0001, 0002, then closes 0001 first (mismatch).
        let content = format!("{}{}{}", ob(1), ob(2), cb(1));
        let report = check_brackets(&content);
        assert!(!report.balanced);
        assert!(report.message.contains("closed bracket doesn't match"));
    }

    #[test]
    fn close_without_any_open_is_unbalanced() {
        let content = cb(1);
        let report = check_brackets(&content);
        assert!(!report.balanced);
        assert!(report.message.contains("closed bracket doesn't match"));
    }

    #[test]
    fn unclosed_open_at_eof_is_unbalanced() {
        let content = ob(1);
        let report = check_brackets(&content);
        assert!(!report.balanced);
        assert!(report
            .message
            .contains("open and closed brackets don't match"));
    }

    #[test]
    fn non_bracket_lines_are_ignored() {
        let content = format!("tx<nu<__________<hello\n{}{}", ob(1), cb(1));
        let report = check_brackets(&content);
        assert!(report.balanced, "{}", report.message);
    }
}
