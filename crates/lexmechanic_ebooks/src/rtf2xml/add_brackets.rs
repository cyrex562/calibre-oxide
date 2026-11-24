//! Port of `old_src/src/calibre/ebooks/rtf2xml/add_brackets.py`
//! (`AddBrackets`).
//!
//! Some control words that are supposed to always appear inside their
//! own bracket group (bold, italics, font-color, ... -- the same
//! [`super::old_rtf::ALLOWABLE`] list `old_rtf.py` checks for) instead
//! appear directly in running text with no bracket at all, in RTF from
//! older/nonstandard producers. This pass wraps every run of such
//! "bare" control words in a synthetic bracket group of its own, so
//! every later pass can rely on the group-scoping invariant holding
//! everywhere.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files -- the temp-file / [`super::copy`] / rename dance around the
//! real pass is pipeline plumbing, not ported here (see
//! [`super::process_tokens::process_tokens`]'s own doc for the same
//! call). Unlike most of this issue's other passes, the transformed
//! content isn't unconditionally kept: `AddBrackets.add_brackets`
//! writes to a temp file, validates ITS OWN OUTPUT with
//! [`super::check_brackets`], and only "commits" (in Python: renames
//! the temp file over the original) if the brackets it just added
//! balance -- otherwise the original content is left untouched and, at
//! `run_level > 0`, a diagnostic is printed. [`add_brackets`] mirrors
//! this exactly: it returns the transformed content on success, or the
//! ORIGINAL `content` unchanged (with the same `eprintln!` diagnostic)
//! when its own output would be unbalanced.

use indexmap::IndexMap;

use super::check_brackets::check_brackets;
use super::old_rtf::ALLOWABLE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeBody,
    InBody,
    AfterControlWord,
    InIgnore,
}

/// Port of `line[:16]`, tolerant of shorter lines (see
/// [`super::check_brackets`]'s helper of the same shape).
fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `line[-5:-1]` (a Python line with its trailing `\n` still
/// attached) applied to a `str::lines()` line (trailing `\n` already
/// stripped): the last 4 characters.
fn last_four(line: &str) -> String {
    if line.len() >= 4 {
        line[line.len() - 4..].to_string()
    } else {
        line.to_string()
    }
}

/// Port of `token_info in self.__accept`: `self.__accept` is
/// `old_rtf.py`'s [`ALLOWABLE`] labels, each spelled out with the full
/// `cw<ci<{label}` prefix.
fn is_accepted(token: &str) -> bool {
    token
        .strip_prefix("cw<ci<")
        .map(|label| ALLOWABLE.contains(&label))
        .unwrap_or(false)
}

/// Port of `__change_permanent_group`: rebuilds `inline` from
/// `temp_group` wholesale (Python reassigns `self.__inline` rather
/// than mutating it) -- `line[:16]` is the map key, `line[20:-1]`
/// (here: `line[20..]`, since `.lines()` already stripped the `\n`
/// Python's slice drops) is the value. The `is_accepted` filter is
/// redundant in practice (every line that ever reaches `temp_group`
/// already passed the same check on the way in), but Python's own
/// literal code has it too -- see `add_brackets.py`'s own
/// rhetorical "Is this really necessary?" comment -- so it's kept
/// rather than "optimized" away.
fn change_permanent_group(temp_group: &[String], inline: &mut IndexMap<String, String>) {
    let mut new_inline = IndexMap::new();
    for line in temp_group {
        let key = token_info(line);
        if is_accepted(key) {
            let value = if line.len() >= 20 {
                line[20..].to_string()
            } else {
                String::new()
            };
            new_inline.insert(key.to_string(), value);
        }
    }
    *inline = new_inline;
}

/// Port of `__write_group`: closes any already-open synthetic bracket,
/// then opens a fresh one containing every `inline` entry whose value
/// isn't the literal string `"false"`.
fn write_group(out: &mut String, open_bracket: &mut bool, inline: &IndexMap<String, String>) {
    if *open_bracket {
        out.push_str("cb<nu<clos-brack<0003\n");
        *open_bracket = false;
    }
    let inline_string: String = inline
        .iter()
        .filter(|(_, v)| v.as_str() != "false")
        .map(|(k, v)| format!("{k}<nu<{v}\n"))
        .collect();
    if !inline_string.is_empty() {
        out.push_str("ob<nu<open-brack<0003\n");
        out.push_str(&inline_string);
        *open_bracket = true;
    }
}

/// Port of `AddBrackets.__init__` + the state-machine body of
/// `add_brackets` (everything up to, but not including, the final
/// `check_brackets`/commit-or-discard decision -- see [`add_brackets`]
/// for that).
fn add_brackets_pass(content: &str) -> String {
    let mut state = State::BeforeBody;
    let mut out = String::new();
    let mut inline: IndexMap<String, String> = IndexMap::new();
    let mut temp_group: Vec<String> = Vec::new();
    let mut open_bracket = false;
    let mut ob_count = String::new();
    let mut cb_count = String::new();
    let mut ignore_count = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if tok == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        match state {
            State::BeforeBody => {
                // Port of `__before_body_func`.
                if tok == "mi<mk<body-open_" {
                    state = State::InBody;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::InBody => {
                // Port of `__in_body_func`.
                if line == "cb<nu<clos-brack<0001" && open_bracket {
                    // The body's OWN final closing bracket: close our
                    // still-open synthetic bracket first, so it
                    // doesn't cross the body's own close.
                    out.push_str("cb<nu<clos-brack<0003\n");
                    out.push_str(line);
                    out.push('\n');
                } else if tok == "ob<nu<open-brack" {
                    state = State::InIgnore;
                    ignore_count = ob_count.clone();
                    out.push_str(line);
                    out.push('\n');
                } else if is_accepted(tok) {
                    temp_group.push(line.to_string());
                    state = State::AfterControlWord;
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            State::AfterControlWord => {
                // Port of `__after_control_word_func`.
                if is_accepted(tok) {
                    temp_group.push(line.to_string());
                } else {
                    change_permanent_group(&temp_group, &mut inline);
                    write_group(&mut out, &mut open_bracket, &inline);
                    temp_group.clear();
                    out.push_str(line);
                    out.push('\n');
                    if tok == "ob<nu<open-brack" {
                        state = State::InIgnore;
                        ignore_count = ob_count.clone();
                    } else {
                        state = State::InBody;
                    }
                }
            }
            State::InIgnore => {
                // Port of `__ignore_func`: copy through unchanged.
                out.push_str(line);
                out.push('\n');
                if tok == "cb<nu<clos-brack" && cb_count == ignore_count {
                    state = State::InBody;
                }
            }
        }
    }
    out
}

/// Port of `AddBrackets.add_brackets`. See this module's own docs for
/// why the return value is either the transformed content (brackets
/// balanced) or `content` unchanged (they didn't) -- `run_level` gates
/// the diagnostic exactly as Python's `run_level > 0` does, on the
/// discard path only.
pub fn add_brackets(content: &str, run_level: u32) -> String {
    let transformed = add_brackets_pass(content);
    if check_brackets(&transformed).balanced {
        transformed
    } else {
        if run_level > 0 {
            eprintln!(
                "Sorry, but this files has a mix of old and new RTF.\n\
                 Some characteristics cannot be converted."
            );
        }
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A self-contained, always-balanced RTF-intermediate document:
    /// the outermost group opens (`0001`, matching `check_brackets`'s
    /// own tracked pairing -- NOT something `add_brackets` itself
    /// opens, unlike the synthetic `0003` groups it may add) BEFORE
    /// the body starts, `inner` runs entirely at the body's own
    /// top-bracket level, and the SAME `0001` group closes at the very
    /// end -- matching how the real pipeline's outermost RTF group
    /// actually spans the whole document.
    fn wrap_body(inner: &str) -> String {
        format!(
            "ob<nu<open-brack<0001\n\
             mi<mk<body-open_\n\
             {inner}\
             cb<nu<clos-brack<0001\n"
        )
    }

    #[test]
    fn wraps_a_single_bare_control_word_run() {
        let content = wrap_body(
            "cw<ci<bold______<nu<true\n\
             tx<nu<__________<hello\n",
        );
        let out = add_brackets(&content, 1);
        // The synthetic group opens right before the bare control word
        // and stays open across the following non-cw line too, closing
        // only once the body's own final bracket forces it shut (see
        // `closes_a_still_open_synthetic_group_before_the_bodys_own_close`).
        assert!(
            out.contains(
                "ob<nu<open-brack<0003\ncw<ci<bold______<nu<true\ntx<nu<__________<hello\ncb<nu<clos-brack<0003\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn drops_a_false_valued_inline_attribute_from_the_synthetic_group() {
        let content = wrap_body("cw<ci<bold______<nu<false\ntx<nu<__________<hello\n");
        let out = add_brackets(&content, 1);
        assert!(
            !out.contains("ob<nu<open-brack<0003"),
            "a false-valued attribute alone shouldn't open a group: {out}"
        );
    }

    #[test]
    fn merges_consecutive_accepted_control_words_into_one_group() {
        let content = wrap_body(
            "cw<ci<bold______<nu<true\n\
             cw<ci<italics___<nu<true\n\
             tx<nu<__________<hi\n",
        );
        let out = add_brackets(&content, 1);
        let open_count = out.matches("ob<nu<open-brack<0003").count();
        assert_eq!(open_count, 1, "one merged group, not two: {out}");
        assert!(out.contains("cw<ci<bold______<nu<true\ncw<ci<italics___<nu<true\n"));
    }

    #[test]
    fn leaves_content_already_inside_a_real_bracket_group_untouched() {
        let content = wrap_body(
            "ob<nu<open-brack<0002\n\
             cw<ci<bold______<nu<true\n\
             cb<nu<clos-brack<0002\n",
        );
        let out = add_brackets(&content, 1);
        // Once a REAL bracket is seen in-body, everything up to its
        // matching close is copied through verbatim (`in_ignore`) --
        // no synthetic group gets added inside it.
        assert!(!out.contains("<0003"), "{out}");
        assert_eq!(out, content);
    }

    #[test]
    fn closes_a_still_open_synthetic_group_before_the_bodys_own_close() {
        // A non-cw line between the bare control word and the body's
        // own close is what returns the state machine to `in_body`
        // (see `__in_body_func`'s `cb<nu<clos-brack<0001` special case
        // in the Python source) -- without it, the still-open group
        // would be caught by `__after_control_word_func` instead and
        // never get closed at all, which is exactly what
        // `discards_the_transform_and_keeps_the_original_when_the_input_is_already_unbalanced`
        // covers.
        let content = wrap_body(
            "cw<ci<bold______<nu<true\n\
             tx<nu<__________<hi\n",
        );
        let out = add_brackets(&content, 1);
        let idx_synthetic_close = out.find("cb<nu<clos-brack<0003").expect("synthetic close");
        let idx_body_close = out.rfind("cb<nu<clos-brack<0001").expect("body close");
        assert!(idx_synthetic_close < idx_body_close, "{out}");
    }

    #[test]
    fn content_before_the_body_is_passed_through_unchanged() {
        let content = "ig<nu<__________<preamble\nmi<mk<body-open_\ncb<nu<clos-brack<0000\n";
        let out = add_brackets(content, 1);
        assert!(out.starts_with("ig<nu<__________<preamble\nmi<mk<body-open_\n"));
    }

    #[test]
    fn discards_the_transform_and_keeps_the_original_when_the_input_is_already_unbalanced() {
        // No real caller would ever feed add_brackets unbalanced
        // input, but the function must still degrade gracefully
        // (keep the original, don't panic) rather than assume balance.
        let content = "mi<mk<body-open_\ncw<ci<bold______<nu<true\ntx<nu<__________<hi\n";
        let out = add_brackets(content, 1);
        assert_eq!(out, content);
    }
}
