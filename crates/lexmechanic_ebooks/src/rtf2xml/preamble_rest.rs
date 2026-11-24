//! Port of `old_src/src/calibre/ebooks/rtf2xml/preamble_rest.py`
//! (`Preamble`).
//!
//! Runs after `info.py`, before `old_rtf.py`, on the bracket-tagged
//! intermediate format described in [`super::process_tokens`]'s module
//! docs. Per the Python docstring, this is a small catch-all cleanup
//! pass for whatever preamble material the more specific earlier passes
//! didn't already consume: it emits one `rtf-definition` tag up front
//! (default font/code page/platform, supplied by the caller -- earlier
//! passes out of scope here resolve these values), and strips any text
//! that leaked into the revision table or list table (both otherwise
//! passed through largely unexamined, pending future support upstream
//! per the docstring's own admission). Unlike every sibling pass ported
//! alongside this one, `Preamble` has no `bug_handler`-raised error path
//! at all -- every branch either writes output or changes state, so
//! [`fix_preamble`] returns a plain `String` rather than a `Result`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Default,
    Revision,
    ListTable,
    Body,
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

/// Port of `Preamble.fix_preamble` (the temp-file / `Copy` / rename
/// dance is pipeline plumbing, not ported here -- see
/// [`super::process_tokens::process_tokens`]'s own doc for the same
/// call). `platform`, `default_font`, and `code_page` mirror the
/// constructor's required arguments of the same names.
pub fn fix_preamble(content: &str, platform: &str, default_font: &str, code_page: &str) -> String {
    let mut state = State::Default;
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        match state {
            State::Default => {
                // Port of `__default_func` + its `__default_dict`
                // dispatch table.
                match tok {
                    "mi<mk<rtfhed-beg" => {
                        // Port of `__found_rtf_head_func`: the marker
                        // line itself is *not* written -- it is
                        // replaced entirely by the synthesized
                        // definition tag.
                        out.push_str(&format!(
                            "mi<tg<empty-att_<rtf-definition<default-font>{default_font}<code-page>{code_page}<platform>{platform}\n"
                        ));
                    }
                    "mi<mk<listabbeg_" => {
                        // Port of `__found_list_table_func`: state
                        // change only, the marker line is dropped.
                        state = State::ListTable;
                    }
                    "mi<mk<revtbl-beg" => {
                        // Port of `__found_revision_table_func`: state
                        // change only, the marker line is dropped.
                        state = State::Revision;
                    }
                    "mi<mk<body-open_" => {
                        // Port of `__found_body_func`: unlike the two
                        // table-start markers above, this one *does*
                        // write the triggering line through.
                        state = State::Body;
                        out.push_str(line);
                        out.push('\n');
                    }
                    _ => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
            State::ListTable => {
                // Port of `__list_table_func`.
                if tok == "mi<mk<listabend_" {
                    state = State::Default;
                } else if line.get(0..2) == Some("tx") {
                    // Bad/unsupported RTF text inside the list table:
                    // dropped, not written.
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            State::Revision => {
                // Port of `__revision_table_func`.
                if tok == "mi<mk<revtbl-end" {
                    state = State::Default;
                } else if line.get(0..2) == Some("tx") {
                    // Text inside the revision table: dropped.
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            State::Body => {
                // Port of `__body_func`: pure passthrough once the
                // body has started.
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_head_marker_becomes_a_definition_tag_and_drops_the_marker() {
        let input = "mi<mk<rtfhed-beg\n";
        let out = fix_preamble(input, "Windows", "0", "ansi1252");
        assert_eq!(
            out,
            "mi<tg<empty-att_<rtf-definition<default-font>0<code-page>ansi1252<platform>Windows\n"
        );
    }

    #[test]
    fn unmarked_default_lines_pass_through_unchanged() {
        let input = "some-line\nanother\n";
        assert_eq!(fix_preamble(input, "Windows", "0", "ansi1252"), input);
    }

    #[test]
    fn body_open_switches_state_and_is_itself_written() {
        let input = "mi<mk<body-open_\ntx<nu<__________<hello\n";
        let out = fix_preamble(input, "Windows", "0", "ansi1252");
        // Both the marker and the following body line pass straight
        // through -- the `Body` state does no filtering at all, unlike
        // `ListTable`/`Revision`.
        assert_eq!(out, input);
    }

    #[test]
    fn list_table_drops_text_but_keeps_other_lines_and_the_marker_is_dropped() {
        let input = concat!(
            "mi<mk<listabbeg_\n",
            "tx<nu<__________<one\n",
            "mi<tg<other\n",
            "mi<mk<listabend_\n",
            "after\n",
        );
        let out = fix_preamble(input, "Windows", "0", "ansi1252");
        assert_eq!(out, "mi<tg<other\nafter\n");
    }

    #[test]
    fn revision_table_drops_text_but_keeps_other_lines_and_the_marker_is_dropped() {
        let input = concat!(
            "mi<mk<revtbl-beg\n",
            "tx<nu<__________<one\n",
            "mi<tg<other\n",
            "mi<mk<revtbl-end\n",
            "after\n",
        );
        let out = fix_preamble(input, "Windows", "0", "ansi1252");
        assert_eq!(out, "mi<tg<other\nafter\n");
    }

    #[test]
    fn full_preamble_sequence() {
        let input = concat!(
            "mi<mk<rtfhed-beg\n",
            "mi<mk<listabbeg_\n",
            "tx<nu<__________<skip-me\n",
            "mi<mk<listabend_\n",
            "mi<mk<body-open_\n",
            "tx<nu<__________<body-text\n",
        );
        let out = fix_preamble(input, "Macintosh", "1", "mac_roman");
        assert_eq!(
            out,
            concat!(
                "mi<tg<empty-att_<rtf-definition<default-font>1<code-page>mac_roman<platform>Macintosh\n",
                "mi<mk<body-open_\n",
                "tx<nu<__________<body-text\n",
            )
        );
    }
}
