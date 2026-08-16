//! Port of `old_src/src/calibre/ebooks/rtf2xml/footnote.py`
//! (`Footnote`).
//!
//! Two public functions, mirroring the Python class's two public
//! methods on one object:
//!
//! - [`separate_footnotes`] (checkpoint `separate_footnotes_info`, run
//!   as this issue's 5th pass): pulls every `\footnote` group's
//!   contents out of the main token stream and appends them, still
//!   individually tagged, to the very end of the content, leaving an
//!   `mi<mk<footnt-ind<NNNN` placeholder marker in the main stream at
//!   each extraction point.
//! - [`join_footnotes`] (called much later in the real pipeline, after
//!   `inline.py`'s body pass -- out of scope here, but the method
//!   itself lives in this same Python file, so it's ported here too):
//!   reverses the process, splicing each footnote's content back in at
//!   its placeholder marker.
//!
//! # Threading `found_a_footnote` between the two calls
//!
//! In the Python, `self.__found_a_footnote` is set by
//! `separate_footnotes` and read by `join_footnotes` (which no-ops
//! entirely, leaving its input untouched, if no footnote was ever
//! found) via shared object state. Modeled here the same way
//! [`super::paragraph_def`]/[`super::body_styles`] thread
//! `list_of_styles`: [`SeparateFootnotesOutput::found_a_footnote`] is a
//! real output the caller must pass into [`join_footnotes`] explicitly.

/// Result of [`separate_footnotes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeparateFootnotesOutput {
    pub content: String,
    /// Port of `self.__found_a_footnote`.
    pub found_a_footnote: bool,
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

/// Port of `Footnote.separate_footnotes`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening files.
pub fn separate_footnotes(content: &str) -> SeparateFootnotesOutput {
    let mut body = String::new();
    let mut foot = String::new();
    let mut in_footnote = false;
    let mut first_line = false;
    let mut footnote_count: u32 = 0;
    let mut footnote_bracket_count = String::new();
    let mut ob_count = String::new();
    let mut cb_count = String::new();
    let mut found_a_footnote = false;

    for line in content.lines() {
        let info = token_info(line);
        if info == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if info == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        if in_footnote {
            // Port of `__in_footnote_func`.
            if first_line {
                // Port of `__first_line_func`.
                if info == "cw<nt<type______" {
                    foot.push_str(&format!(
                        "mi<tg<open-att__<footnote<type>endnote<num>{footnote_count}\n"
                    ));
                } else {
                    foot.push_str(&format!("mi<tg<open-att__<footnote<num>{footnote_count}\n"));
                }
                first_line = false;
            }
            if info == "cw<ci<footnot-mk" {
                foot.push_str(line);
                foot.push('\n');
                foot.push_str(&format!("tx<nu<__________<{footnote_count}\n"));
            }
            if cb_count == footnote_bracket_count {
                in_footnote = false;
                body.push_str(line);
                body.push('\n');
                foot.push_str("mi<mk<foot___clo\n");
                foot.push_str("mi<tg<close_____<footnote\n");
                foot.push_str("mi<mk<footnt-clo\n");
            } else {
                foot.push_str(line);
                foot.push('\n');
            }
        } else {
            // Port of `__default_sep`.
            if info == "cw<nt<footnote__" {
                // Port of `__found_footnote`.
                found_a_footnote = true;
                in_footnote = true;
                first_line = true;
                footnote_count += 1;
                cb_count = "0".to_string();
                footnote_bracket_count = ob_count.clone();
                body.push_str(&format!("mi<mk<footnt-ind<{footnote_count:04}\n"));
                foot.push_str(&format!("mi<mk<footnt-ope<{footnote_count:04}\n"));
            }
            body.push_str(line);
            body.push('\n');
            if info == "cw<ci<footnot-mk" {
                body.push_str(&format!("tx<nu<__________<{}\n", footnote_count + 1));
            }
        }
    }

    body.push_str(
        "mi<mk<sect-close\n\
         mi<mk<body-close\n\
         mi<tg<close_____<section\n\
         mi<tg<close_____<body\n\
         mi<tg<close_____<doc\n\
         mi<mk<footnt-beg\n",
    );
    body.push_str(&foot);
    body.push_str("mi<mk<footnt-end\n");

    SeparateFootnotesOutput {
        content: body,
        found_a_footnote,
    }
}

/// Port of `Footnote.join_footnotes`. `found_a_footnote` is
/// [`SeparateFootnotesOutput::found_a_footnote`] from the matching
/// [`separate_footnotes`] call -- if `false`, this is a no-op returning
/// `content` unchanged (matching the Python's early `return`, which
/// leaves the trailing `footnt-beg`/`footnt-end` markers -- and their
/// empty footnote payload -- sitting in the stream un-collapsed; this
/// is intentional pipeline behavior, not a bug, since a later
/// out-of-scope pass is expected to deal with the always-appended
/// trailer block regardless of whether any footnote was ever found).
pub fn join_footnotes(content: &str, found_a_footnote: bool) -> String {
    if !found_a_footnote {
        return content.to_string();
    }

    // Port of `__get_footnotes`: split `content` into the body (with
    // `mi<mk<footnt-ind<NNNN` placeholders still in place) and the
    // footnote payload (the material between `footnt-beg`/`footnt-end`).
    let mut body_no_foot = String::new();
    let mut foot_payload = String::new();
    let mut in_foot = false;
    for line in content.lines() {
        let info = token_info(line);
        if in_foot {
            if info == "mi<mk<footnt-end" {
                in_foot = false;
            } else {
                foot_payload.push_str(line);
                foot_payload.push('\n');
            }
        } else if info == "mi<mk<footnt-beg" {
            in_foot = true;
        } else {
            body_no_foot.push_str(line);
            body_no_foot.push('\n');
        }
    }

    // Port of `__get_foot_from_temp`: for a given 4-digit number, find
    // the `mi<mk<footnt-ope<NNNN` marker in the footnote payload and
    // return everything up to (not including) the matching
    // `mi<mk<footnt-clo` line.
    let get_foot = |num: &str| -> Option<String> {
        let look_for = format!("mi<mk<footnt-ope<{num}");
        let mut found = false;
        let mut collected = String::new();
        for line in foot_payload.lines() {
            if found {
                if line == "mi<mk<footnt-clo" {
                    return Some(collected);
                }
                collected.push_str(line);
                collected.push('\n');
            } else if line == look_for {
                found = true;
            }
        }
        None
    };

    // Port of `__join_from_temp`.
    let mut out = String::new();
    for line in body_no_foot.lines() {
        if token_info(line) == "mi<mk<footnt-ind" {
            // Port of `line[17:-1]`.
            let num = if line.len() > 17 { &line[17..] } else { "" };
            if let Some(footnote_text) = get_foot(num) {
                out.push_str(&footnote_text);
                continue;
            }
            // Port of `__get_foot_from_temp` falling off its loop
            // without ever finding `look_for` or without a matching
            // close marker: it implicitly returns `None`, and the
            // Python then writes that `None` with `write_obj.write`,
            // which would raise `TypeError` -- an unreachable path for
            // any input `separate_footnotes` itself produced (every
            // `footnt-ind` marker it writes has a matching
            // `footnt-ope`/`footnt-clo` pair), so not modeled as a
            // real error path here; a missing/unmatched marker is
            // instead just dropped.
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn no_footnotes_still_appends_empty_trailer() {
        let content = "tx<nu<__________<hello\n";
        let out = separate_footnotes(content);
        assert!(!out.found_a_footnote);
        assert!(out.content.starts_with("tx<nu<__________<hello\n"));
        assert!(out.content.contains("mi<mk<footnt-beg\nmi<mk<footnt-end\n"));
    }

    #[test]
    fn footnote_group_is_extracted_with_marker_left_behind() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<nt<footnote__<nu<true",
            "tx<nu<__________<a footnote",
            "cb<nu<clos-brack<0001",
        ]);
        let out = separate_footnotes(&content);
        assert!(out.found_a_footnote);
        assert!(out.content.contains("mi<mk<footnt-ind<0001"));
        assert!(out.content.contains("mi<mk<footnt-ope<0001"));
        assert!(out.content.contains("mi<tg<open-att__<footnote<num>1"));
        assert!(out.content.contains("a footnote"));
        assert!(out.content.contains("mi<tg<close_____<footnote"));
    }

    #[test]
    fn endnote_type_gets_endnote_attribute() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<nt<footnote__<nu<true",
            "cw<nt<type______<nu<endnote",
            "cb<nu<clos-brack<0001",
        ]);
        let out = separate_footnotes(&content);
        assert!(out
            .content
            .contains("mi<tg<open-att__<footnote<type>endnote<num>1"));
    }

    #[test]
    fn join_footnotes_is_a_no_op_when_none_were_found() {
        let content = "tx<nu<__________<unchanged\n";
        assert_eq!(join_footnotes(content, false), content);
    }

    #[test]
    fn round_trip_separate_then_join_restores_original_shape() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "tx<nu<__________<before ",
            &{
                let s = "ob<nu<open-brack<0002".to_string();
                s
            },
            "cw<nt<footnote__<nu<true",
            "tx<nu<__________<footnote body",
            "cb<nu<clos-brack<0002",
            "tx<nu<__________< after",
            "cb<nu<clos-brack<0001",
        ]);
        let sep = separate_footnotes(&content);
        assert!(sep.found_a_footnote);
        let joined = join_footnotes(&sep.content, sep.found_a_footnote);
        // the footnote body text ends up back in the main stream, and
        // the trailer/placeholder machinery is fully consumed.
        assert!(joined.contains("footnote body"));
        assert!(!joined.contains("footnt-ind"));
        assert!(!joined.contains("footnt-beg"));
        assert!(!joined.contains("footnt-end"));
        assert!(joined.contains("before"));
        assert!(joined.contains("after"));
    }

    #[test]
    fn multiple_footnotes_are_joined_back_at_their_own_markers() {
        let content = lines(&[
            "ob<nu<open-brack<0001",
            "cw<nt<footnote__<nu<true",
            "tx<nu<__________<first",
            "cb<nu<clos-brack<0001",
            "tx<nu<__________<middle",
            "ob<nu<open-brack<0002",
            "cw<nt<footnote__<nu<true",
            "tx<nu<__________<second",
            "cb<nu<clos-brack<0002",
        ]);
        let sep = separate_footnotes(&content);
        let joined = join_footnotes(&sep.content, sep.found_a_footnote);
        let first_pos = joined.find("first").unwrap();
        let middle_pos = joined.find("middle").unwrap();
        let second_pos = joined.find("second").unwrap();
        assert!(first_pos < middle_pos);
        assert!(middle_pos < second_pos);
    }
}
