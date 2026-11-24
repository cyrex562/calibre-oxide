//! Port of `old_src/src/calibre/ebooks/rtf2xml/delete_info.py`
//! (`DeleteInfo`).
//!
//! Despite the name (and a thematic echo with [`super::info`], which
//! also touches the RTF `\info` document-info group -- see that
//! module's own doc comment for exactly how the two relate, or rather
//! don't, in the real pipeline), `DeleteInfo` is not really about
//! document info at all. It's a generic pass over MS Word's
//! "destination group" convention: a group `{\*\keyword ...}` is
//! marked ignorable/discardable by the leading `\*` unless the reader
//! specifically recognizes `\keyword`. This pass walks every such
//! group and either keeps its contents (if `\keyword` is on a fixed
//! allow-list, or is the special `\*\pn` old-style list marker, whose
//! *contents* -- but not its own brackets -- are kept) or deletes the
//! whole group.
//!
//! Checkpoint `delete_data_info` in `ParseRtf.py`; runs first among
//! this issue's 18 passes, immediately after the (out of scope)
//! `check_encoding`/`default_encoding` preamble-detection step.

use thiserror::Error;

/// Port of the `raise self.__bug_handler(msg)` calls in
/// `__asterisk_func`, both gated by `run_level` thresholds the Python
/// checks before raising (below the threshold, the Python instead just
/// falls through -- see each call site in [`asterisk_func`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeleteInfoError {
    /// Port of `'Flag problem\n'`, raised (`run_level > 3`) when a
    /// `{\*}` empty-destination group's closing bracket arrives but the
    /// tracked delete-count doesn't match the current close-bracket
    /// count -- the Python's own comment calls this "not sure what
    /// happens here! believe I have a `{\*}`", i.e. an already-odd edge
    /// case even to the original author.
    #[error("Flag problem\n")]
    FlagProblem,
    /// Port of the `run_level > 5` gated raise in the final `else`
    /// branch of `__asterisk_func`: an unrecognized `\*` destination
    /// keyword. Message reproduced verbatim, including the source's
    /// backslash-newline line-continuation (which Python's string
    /// literal escaping removes, leaving the literal indentation
    /// whitespace from the next source line embedded in the message).
    #[error(
        "After an asterisk, and found neither an allowable or non-allowable token\n                            token is \"{0}\"\n"
    )]
    UnrecognizedDestination(String),
}

pub type Result<T> = std::result::Result<T, DeleteInfoError>;

/// Result of [`delete_info`]: the transformed content plus the
/// Python's `self.__found_delete` (`True` if any `\*` destination
/// group was ever encountered, whether or not it was actually
/// deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteInfoOutput {
    pub content: String,
    pub found_delete: bool,
}

/// Control words allowed to survive inside a `{\*\keyword ...}`
/// destination group -- port of `__allowable`.
const ALLOWABLE: &[&str] = &[
    "cw<ss<char-style",
    "cw<it<listtable_",
    "cw<it<revi-table",
    "cw<ls<list-lev-d",
    "cw<fd<field-inst",
    "cw<an<book-mk-st",
    "cw<an<book-mk-en",
    "cw<an<annotation",
    "cw<cm<comment___",
    "cw<it<lovr-table",
    "cw<di<company___",
];

/// Explicitly-always-deleted destination keywords -- port of
/// `__not_allowable`.
const NOT_ALLOWABLE: &[&str] = &[
    "cw<un<unknown___",
    "cw<un<company___",
    "cw<ls<list-level",
    "cw<fd<datafield_",
];

/// Port of `DeleteInfo`'s per-call state, threaded explicitly here
/// instead of as `self` fields.
struct State {
    state: Stage,
    /// Port of `self.__ob`: `None` (Python `0`) or the most recently
    /// seen, not-yet-resolved `ob<nu<open-brack` line, buffered while
    /// we wait to see whether the *next* token is an asterisk (i.e.
    /// whether this bracket opens a destination group).
    ob: Option<String>,
    /// Port of `self.__write_cb`.
    write_cb: bool,
    found_delete: bool,
    /// Port of `self.__ob_count` / `self.__delete_count` /
    /// `self.__cb_count`: 4-digit bracket-nesting sequence numbers,
    /// compared as strings exactly as the Python does (both sides are
    /// always populated from the same `line[-5:-1]` 4-digit-zero-padded
    /// slice by the time a real comparison matters -- the Python's
    /// initial `int` `0` for `__ob_count`/`__cb_count` before any
    /// bracket is seen is not reproduced as a distinct type here since
    /// no well-formed RTF fixture can reach an asterisk before its
    /// enclosing `{`).
    ob_count: String,
    cb_count: String,
    delete_count: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Default,
    AfterAsterisk,
    Delete,
    List,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 {
        line[line.len() - 4..].to_string()
    } else {
        line.to_string()
    }
}

/// Port of `__default_func`. Returns `(write_current_line, flushed)`
/// where `flushed` is an already-buffered previous line to write out
/// ahead of the current line's own write decision.
fn default_func(st: &mut State, line: &str, info: &str) -> (bool, Option<String>) {
    if info == "cw<ml<asterisk__" {
        st.state = Stage::AfterAsterisk;
        st.delete_count = st.ob_count.clone();
        (false, None)
    } else if info == "ob<nu<open-brack" {
        let flushed = st.ob.take();
        st.ob = Some(line.to_string());
        (false, flushed)
    } else {
        let flushed = st.ob.take();
        (true, flushed)
    }
}

/// Port of `__asterisk_func`.
fn asterisk_func(
    st: &mut State,
    _line: &str,
    info: &str,
    run_level: u32,
) -> Result<(bool, Option<String>)> {
    st.found_delete = true;
    if info == "cb<nu<clos-brack" {
        if st.delete_count == st.cb_count {
            st.state = Stage::Default;
            st.ob = None;
            return Ok((false, None));
        }
        if run_level > 3 {
            return Err(DeleteInfoError::FlagProblem);
        }
        return Ok((true, None));
    } else if ALLOWABLE.contains(&info) {
        let mut flushed = None;
        if let Some(ob) = st.ob.take() {
            flushed = Some(ob);
            st.state = Stage::Default;
        }
        return Ok((true, flushed));
    } else if info == "cw<ls<list______" {
        st.ob = None;
        st.state = Stage::List;
        return Ok((false, None));
    } else if NOT_ALLOWABLE.contains(&info) {
        if st.ob.is_none() {
            st.write_cb = true;
        }
        st.ob = None;
        st.state = Stage::Delete;
        st.cb_count = "0".to_string();
        return Ok((false, None));
    }
    if run_level > 5 {
        return Err(DeleteInfoError::UnrecognizedDestination(info.to_string()));
    }
    if st.ob.is_none() {
        st.write_cb = true;
    }
    st.ob = None;
    st.state = Stage::Delete;
    st.cb_count = "0".to_string();
    Ok((false, None))
}

/// Port of `__delete_func`.
///
/// # Preserved upstream quirk: `write_cb` is never actually reset here
///
/// `if self.__write_cb: self.__write_cb = True; return True` -- the
/// reassignment is a no-op (`True = True`); the equivalent branch in
/// `__list_func` (identical shape otherwise) correctly uses `False`.
/// Ported verbatim as [`delete_func`]'s equally-inert
/// `st.write_cb = true` below -- see
/// `delete_func_leaves_write_cb_set_unlike_list_func` for a direct
/// demonstration against the state-transition functions. Whether this
/// is reachable via realistic tokenized input is unclear even after
/// tracing every branch that can set `write_cb = true`
/// (`asterisk_func`'s not-allowable/unrecognized branches, gated on
/// `self.__ob` already being falsy) against every branch that could
/// flush `self.__ob` to falsy beforehand -- every path this port could
/// construct leaves `self.__ob` truthy at that point, so the bug may be
/// dormant in practice. Kept as-is regardless, matching the original
/// author's own "not sure what happens here!" comment elsewhere in
/// this same file about a neighboring edge case.
fn delete_func(st: &mut State) -> bool {
    if st.delete_count == st.cb_count {
        st.state = Stage::Default;
        if st.write_cb {
            st.write_cb = true; // preserved no-op, see doc comment above
            return true;
        }
        return false;
    }
    false
}

/// Port of `__list_func`.
fn list_func(st: &mut State, info: &str, line: &str) -> bool {
    if st.delete_count == st.cb_count && info == "cb<nu<clos-brack" {
        st.state = Stage::Default;
        if st.write_cb {
            st.write_cb = false;
            return true;
        }
        return false;
    } else if line.len() >= 2 && &line[..2] == "cw" {
        return true;
    }
    false
}

/// Port of `DeleteInfo.delete_info`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file.
pub fn delete_info(content: &str, run_level: u32) -> Result<DeleteInfoOutput> {
    let mut st = State {
        state: Stage::Default,
        ob: None,
        write_cb: false,
        found_delete: false,
        ob_count: "0".to_string(),
        cb_count: "0".to_string(),
        delete_count: "0".to_string(),
    };
    let mut out = String::new();

    for line in content.lines() {
        let info = token_info(line);
        if info == "ob<nu<open-brack" {
            st.ob_count = last_four(line);
        }
        if info == "cb<nu<clos-brack" {
            st.cb_count = last_four(line);
        }

        let (write_current, flushed) = match st.state {
            Stage::Default => default_func(&mut st, line, info),
            Stage::AfterAsterisk => asterisk_func(&mut st, line, info, run_level)?,
            Stage::Delete => (delete_func(&mut st), None),
            Stage::List => (list_func(&mut st, info, line), None),
        };
        if let Some(flushed) = flushed {
            out.push_str(&flushed);
            out.push('\n');
        }
        if write_current {
            out.push_str(line);
            out.push('\n');
        }
    }

    Ok(DeleteInfoOutput {
        content: out,
        found_delete: st.found_delete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtf2xml::check_brackets;

    fn ob(n: u32) -> String {
        format!("ob<nu<open-brack<{n:04}")
    }
    fn cb(n: u32) -> String {
        format!("cb<nu<clos-brack<{n:04}")
    }

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\n";
        let out = delete_info(content, 1).unwrap();
        assert_eq!(out.content, content);
        assert!(!out.found_delete);
    }

    #[test]
    fn brackets_with_no_asterisk_pass_through() {
        let content = lines(&[&ob(1), "tx<nu<__________<hi", &cb(1)]);
        let out = delete_info(&content, 1).unwrap();
        assert_eq!(out.content, content);
        assert!(!out.found_delete);
    }

    #[test]
    fn allowable_destination_group_is_kept() {
        // {\*\annotation some text}
        let content = lines(&[
            &ob(1),
            "cw<ml<asterisk__<nu<true",
            "cw<an<annotation<nu<true",
            "tx<nu<__________<note",
            &cb(1),
        ]);
        let out = delete_info(&content, 1).unwrap();
        assert!(out.found_delete);
        // opening bracket, the annotation control word, the text, and
        // the closing bracket all survive; only the asterisk marker
        // itself is dropped.
        assert_eq!(
            out.content,
            lines(&[
                &ob(1),
                "cw<an<annotation<nu<true",
                "tx<nu<__________<note",
                &cb(1)
            ])
        );
    }

    #[test]
    fn not_allowable_destination_group_is_deleted_entirely() {
        // {\*\datafield junk}
        let content = lines(&[
            &ob(1),
            "cw<ml<asterisk__<nu<true",
            "cw<fd<datafield_<nu<true",
            "tx<nu<__________<junk",
            &cb(1),
        ]);
        let out = delete_info(&content, 1).unwrap();
        assert!(out.found_delete);
        assert_eq!(out.content, "");
    }

    #[test]
    fn unrecognized_destination_deleted_by_default_below_run_level_six() {
        let content = lines(&[
            &ob(1),
            "cw<ml<asterisk__<nu<true",
            "cw<zz<mystery___<nu<true",
            &cb(1),
        ]);
        let out = delete_info(&content, 5).unwrap();
        assert!(out.found_delete);
        assert_eq!(out.content, "");
    }

    #[test]
    fn unrecognized_destination_raises_above_run_level_five() {
        let content = lines(&[
            &ob(1),
            "cw<ml<asterisk__<nu<true",
            "cw<zz<mystery___<nu<true",
            &cb(1),
        ]);
        let err = delete_info(&content, 6).unwrap_err();
        assert!(
            matches!(err, DeleteInfoError::UnrecognizedDestination(t) if t == "cw<zz<mystery___")
        );
    }

    #[test]
    fn empty_asterisk_group_vanishes_with_no_flag_problem() {
        // {\*}
        let content = lines(&[&ob(1), "cw<ml<asterisk__<nu<true", &cb(1)]);
        let out = delete_info(&content, 10).unwrap();
        assert!(out.found_delete);
        assert_eq!(out.content, "");
    }

    #[test]
    fn old_style_list_marker_keeps_control_words_but_drops_its_own_brackets() {
        // {\*\pn ...} -- \pn resolves to cw<ls<list______ per
        // process_tokens.rs's dict_token.
        let content = lines(&[
            &ob(1),
            "cw<ml<asterisk__<nu<true",
            "cw<ls<list______<nu<true",
            "cw<ls<list-start<nu<1",
            "tx<nu<__________<dropped text",
            &cb(1),
        ]);
        let out = delete_info(&content, 1).unwrap();
        assert!(out.found_delete);
        // the group's own `{`/`}`, the `cw<ls<list______` marker line
        // itself (dropped exactly like the asterisk marker -- see
        // `asterisk_func`'s `"cw<ls<list______"` branch, which never
        // returns a truthy write signal for its own current line), and
        // any non-`cw` line (the plain text) are all dropped; only the
        // *other* `cw` tokens inside survive.
        assert_eq!(out.content, lines(&["cw<ls<list-start<nu<1"]));
    }

    /// Demonstrates the `write_cb` self-assignment-no-op quirk
    /// documented on [`delete_func`] directly against the private
    /// state-transition functions (rather than via a full
    /// `delete_info` token stream): tracing the reachable control flow
    /// through `default_func`/`asterisk_func`, `self.__ob` in the
    /// Python is always truthy by the time `__delete_func`'s
    /// `self.__write_cb = True` branch is reached for any well-formed
    /// tokenized input this port's fixtures can construct (every
    /// `{\*...}` destination's own opening bracket is buffered, never
    /// flushed, until it's resolved) -- so the `if not self.__ob:
    /// self.__write_cb = True` branch that sets the flag in the first
    /// place appears unreachable via normal input, matching the
    /// original author's own "not sure what happens here!" comment
    /// elsewhere in this same file. The self-assignment bug is
    /// preserved verbatim regardless (it's what the literal source
    /// does), and is demonstrated here by directly driving the state
    /// machine to the one state `__delete_func`/`__list_func` actually
    /// disagree about: closing a delete-group while `write_cb` is set.
    #[test]
    fn delete_func_leaves_write_cb_set_unlike_list_func() {
        let mut delete_state = State {
            state: Stage::Delete,
            ob: None,
            write_cb: true,
            found_delete: true,
            ob_count: "0001".to_string(),
            cb_count: "0001".to_string(),
            delete_count: "0001".to_string(),
        };
        let wrote = delete_func(&mut delete_state);
        assert!(wrote, "the stray close bracket is written");
        assert!(
            delete_state.write_cb,
            "quirk: __delete_func's `self.__write_cb = True` is a no-op, so \
             the flag leaks into whatever the pass evaluates next"
        );

        let mut list_state = State {
            state: Stage::List,
            ob: None,
            write_cb: true,
            found_delete: true,
            ob_count: "0001".to_string(),
            cb_count: "0001".to_string(),
            delete_count: "0001".to_string(),
        };
        let wrote = list_func(&mut list_state, "cb<nu<clos-brack", "cb<nu<clos-brack<0001");
        assert!(wrote, "the stray close bracket is written here too");
        assert!(
            !list_state.write_cb,
            "__list_func's equivalent branch correctly resets the flag to false"
        );
    }

    #[test]
    fn output_is_balanced_or_intentionally_unbalanced_matches_check_brackets() {
        // A well-formed document with no destination groups at all
        // should still check out as balanced.
        let content = lines(&[&ob(1), &ob(2), &cb(2), &cb(1)]);
        let out = delete_info(&content, 1).unwrap();
        assert!(check_brackets::check_brackets(&out.content).balanced);
    }
}
