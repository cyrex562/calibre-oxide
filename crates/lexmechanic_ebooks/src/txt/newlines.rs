//! Port of `old_src/src/calibre/ebooks/txt/newlines.py`.
//!
//! `'system'` resolves to Python's `os.linesep`, which is `\r\n` on
//! Windows and `\n` everywhere else. This crate's build docs
//! (`docs/AGENT_PORTING_GUIDE.md`) target Linux, and nothing else in
//! this crate carries a per-platform newline table, so `'system'`
//! resolves to a fixed `\n` here -- the same value `os.linesep` would
//! give on the Linux/macOS hosts calibre itself mostly runs on, and the
//! only fixed choice that doesn't require inventing new
//! platform-detection machinery just for this one option.

/// Port of `TxtNewlines`: resolves a newline-style name (looked up
/// case-insensitively, port of `NEWLINE_TYPES.get(newline_type.lower(),
/// os.linesep)`) to the literal newline string it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxtNewlines {
    pub newline: &'static str,
}

impl TxtNewlines {
    /// Unrecognized values fall back to the `'system'` value, exactly as
    /// the Python dict's `.get(..., os.linesep)` default does.
    pub fn new(newline_type: &str) -> Self {
        let newline = match newline_type.to_lowercase().as_str() {
            "unix" => "\n",
            "old_mac" => "\r",
            "windows" => "\r\n",
            // "system" and anything unrecognized.
            _ => "\n",
        };
        TxtNewlines { newline }
    }
}

/// Port of `specified_newlines`: normalize every newline style to `\n`,
/// then convert to `newline` (a no-op when `newline` is already `\n`).
pub fn specified_newlines(newline: &str, text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if newline == "\n" {
        return text;
    }
    text.replace('\n', newline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_each_named_style() {
        assert_eq!(TxtNewlines::new("unix").newline, "\n");
        assert_eq!(TxtNewlines::new("old_mac").newline, "\r");
        assert_eq!(TxtNewlines::new("windows").newline, "\r\n");
        assert_eq!(TxtNewlines::new("system").newline, "\n");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(TxtNewlines::new("WINDOWS").newline, "\r\n");
        assert_eq!(TxtNewlines::new("Old_Mac").newline, "\r");
    }

    #[test]
    fn unrecognized_value_falls_back_to_system() {
        assert_eq!(TxtNewlines::new("bogus").newline, "\n");
    }

    #[test]
    fn specified_newlines_normalizes_then_converts() {
        let mixed = "a\r\nb\rc\nd";
        assert_eq!(specified_newlines("\n", mixed), "a\nb\nc\nd");
        assert_eq!(specified_newlines("\r\n", mixed), "a\r\nb\r\nc\r\nd");
        assert_eq!(specified_newlines("\r", mixed), "a\rb\rc\rd");
    }

    #[test]
    fn specified_newlines_is_a_noop_for_unix_input() {
        assert_eq!(specified_newlines("\n", "already\nunix"), "already\nunix");
    }
}
