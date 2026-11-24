//! Port of `old_src/src/calibre/ebooks/rtf2xml/old_rtf.py` (`OldRtf`).
//!
//! Heuristically detects older/nonstandard RTF producers: certain
//! character-formatting control words (bold, italics, font-color, ...)
//! are supposed to always appear inside their own group (so their
//! effect is properly scoped and later closed), but some older RTF
//! writers emit them directly in running text at the top body-bracket
//! level. If any allow-listed keyword is found at exactly the body's
//! own bracket depth (not inside a nested group), the document is
//! flagged old-style.
//!
//! Unlike this issue's other 17 passes, `check_if_old_rtf` doesn't
//! transform the content -- it only *inspects* it and returns a bool.
//! In the real `ParseRtf.py`, the call itself
//! (`old_rtf_obj.check_if_old_rtf()`) is unconditional; it's only the
//! *follow-up* actions (raising above `run_level > 5`, or printing a
//! `run_level > 1`-gated diagnostic) that are conditional -- see this
//! module's own [`check_if_old_rtf`] for the run-level-gated diagnostic
//! this port preserves, and `ParseRtf.py` (out of scope) for the
//! higher-level `run_level > 5` raise built on top of this function's
//! return value.

/// Control-word labels (the fixed 10-char part of `cw<ci<{label}` and a
/// few other categories) that must always be inside their own group --
/// port of `__allowable`. `pub(crate)`: `add_brackets.py`'s own
/// `__accept` list (see [`super::add_brackets`]) is this SAME 25-entry
/// list verbatim (confirmed by reading both `old_src` files side by
/// side, not assumed from the name) -- kept as one shared constant
/// here rather than a second, driftable copy.
pub(crate) const ALLOWABLE: &[&str] = &[
    "annotation",
    "blue______",
    "bold______",
    "caps______",
    "char-style",
    "dbl-strike",
    "emboss____",
    "engrave___",
    "font-color",
    "font-down_",
    "font-size_",
    "font-style",
    "font-up___",
    "footnot-mk",
    "green_____",
    "hidden____",
    "italics___",
    "outline___",
    "red_______",
    "shadow____",
    "small-caps",
    "strike-thr",
    "subscript_",
    "superscrip",
    "underlined",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    BeforeBody,
    InBody,
    AfterPard,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `Header`/`Footnote`-style `line[6:16]` slicing: the
/// category-specific label following `cw<{pre}<`.
fn inline_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[6..16]
    } else {
        ""
    }
}

/// Port of `OldRtf.check_if_old_rtf`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file. `run_level` only gates whether a
/// diagnostic is printed via `eprintln!` when old RTF is detected (port
/// of the Python's `sys.stderr.write`); it never changes the returned
/// bool.
pub fn check_if_old_rtf(content: &str, run_level: u32) -> bool {
    check_if_old_rtf_impl(content, run_level)
}

fn check_if_old_rtf_impl(content: &str, run_level: u32) -> bool {
    let mut stage = Stage::BeforeBody;
    let mut ob_group: i64 = 0;
    let mut base_ob_count: i64 = 0;
    let mut found_new_in_this_group: i64 = 0;

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let info = token_info(line);

        if info == "mi<mk<body-close" {
            return false;
        }
        if info == "ob<nu<open-brack" {
            ob_group += 1;
        }
        if info == "cb<nu<clos-brack" {
            ob_group -= 1;
        }
        let inline = inline_info(line);

        // Port of `if self.__state == 'after_body': return False`.
        // `'after_body'` is never actually assigned anywhere in the
        // Python (only `'before_body'`/`'in_body'`/`'after_pard'`
        // are), so this branch is dead code, preserved here only as
        // this comment rather than an unreachable enum variant --
        // Rust's exhaustive `match` below already makes the omission
        // sound.

        let result = match stage {
            Stage::BeforeBody => {
                // Port of `__before_body_func`.
                if info == "mi<mk<body-open_" {
                    stage = Stage::InBody;
                    base_ob_count = ob_group;
                }
                None
            }
            Stage::InBody => {
                // Port of `__check_tokens_func`. Its `if result ==
                // 'new_rtf'` caller-side check is dead: this function
                // never returns that literal, only `Some("old_rtf")`
                // or `None` (falling through in the Python).
                if ALLOWABLE.contains(&inline) {
                    if ob_group == base_ob_count {
                        Some("old_rtf")
                    } else {
                        found_new_in_this_group += 1;
                        None
                    }
                } else if info == "cw<pf<par-def___" {
                    stage = Stage::AfterPard;
                    None
                } else {
                    None
                }
            }
            Stage::AfterPard => {
                // Port of `__after_pard_func`.
                if line.len() < 2 || &line[..2] != "cw" {
                    stage = Stage::InBody;
                }
                None
            }
        };

        if result == Some("old_rtf") {
            if run_level > 3 {
                eprintln!("Old rtf construction {inline} (bracket {ob_group}, line {line_num})");
            }
            return true;
        }
    }
    let _ = found_new_in_this_group; // mirrors `self.__found_new`, write-only in the Python too
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn empty_content_is_not_old_rtf() {
        assert!(!check_if_old_rtf("", 1));
    }

    #[test]
    fn well_formed_body_with_grouped_formatting_is_not_old() {
        let content = lines(&[
            "mi<mk<body-open_",
            "ob<nu<open-brack<0001",
            "cw<ci<bold______<nu<true",
            "tx<nu<__________<hi",
            "cb<nu<clos-brack<0001",
            "mi<mk<body-close",
        ]);
        assert!(!check_if_old_rtf(&content, 1));
    }

    #[test]
    fn allowable_token_at_body_bracket_depth_is_old_rtf() {
        let content = lines(&[
            "mi<mk<body-open_",
            "cw<ci<bold______<nu<true",
            "tx<nu<__________<hi",
            "mi<mk<body-close",
        ]);
        assert!(check_if_old_rtf(&content, 1));
    }

    #[test]
    fn body_close_before_any_allowable_token_short_circuits_false() {
        let content = lines(&["mi<mk<body-open_", "mi<mk<body-close"]);
        assert!(!check_if_old_rtf(&content, 1));
    }

    #[test]
    fn par_def_transitions_to_after_pard_then_back_to_in_body_on_non_cw_line() {
        // After \pard, a non-cw line (e.g. plain text) returns to
        // in_body state; a subsequent bare bold at body depth is then
        // still detected.
        let content = lines(&[
            "mi<mk<body-open_",
            "cw<pf<par-def___<nu<true",
            "tx<nu<__________<some text",
            "cw<ci<bold______<nu<true",
            "mi<mk<body-close",
        ]);
        assert!(check_if_old_rtf(&content, 1));
    }

    #[test]
    fn cw_lines_immediately_after_pard_stay_in_after_pard_state() {
        // Consecutive cw<pf<...> lines right after \pard don't yet
        // transition back to in_body, so a bare bold appearing in that
        // same run isn't (yet) evaluated as old-rtf.
        let content = lines(&[
            "mi<mk<body-open_",
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<body-close",
        ]);
        assert!(!check_if_old_rtf(&content, 1));
    }

    #[test]
    fn nested_group_formatting_does_not_trigger_old_rtf() {
        let content = lines(&[
            "mi<mk<body-open_",
            "ob<nu<open-brack<0001",
            "ob<nu<open-brack<0002",
            "cw<ci<italics___<nu<true",
            "cb<nu<clos-brack<0002",
            "cb<nu<clos-brack<0001",
            "mi<mk<body-close",
        ]);
        assert!(!check_if_old_rtf(&content, 1));
    }
}
