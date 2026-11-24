//! Port of `old_src/src/calibre/ebooks/rtf2xml/combine_borders.py`
//! (`CombineBorders`).
//!
//! Merges a border's `cw<bd<{border-position}<...` opening line and the
//! run of `cw<bt<{border-style-attribute}<...` lines that follow it
//! (border thickness, color, style, etc. -- resolved from RTF's
//! `\brdrs`/`\brdrw`/`\brdrcf`/... keywords, see [`super::process_tokens`]'s
//! `bt` category) into a single combined
//! `cw<bd<{border-position}<nu<{attr1}|{attr2}:{val2}|...` line, so
//! later passes (in particular [`super::paragraph_def`]'s private
//! border-parsing helper) can read one line per border instead of
//! walking a variable-length run.

/// Port of `CombineBorders`'s per-call state.
struct State {
    in_border: bool,
    bord_pos: String,
    bord_att: Vec<String>,
}

fn first_five(line: &str) -> &str {
    if line.len() >= 5 {
        &line[..5]
    } else {
        line
    }
}

/// Port of `add_to_border_desc`: `line[6:16]` is the `bt` subtype
/// label, `line[20:-1]` its value (trailing `\n` already stripped
/// here, so no `-1`).
fn add_to_border_desc(st: &mut State, line: &str) {
    let border_desc = if line.len() >= 16 { &line[6..16] } else { "" };
    let value = if line.len() > 20 { &line[20..] } else { "" };
    let suffix = if value == "true" {
        String::new()
    } else {
        format!(":{value}")
    };
    st.bord_att.push(format!("{border_desc}{suffix}"));
}

/// Port of `found_bd`: `line[6:16]` is the `bd` subtype label (border
/// position, e.g. `bor-t-r-vi`).
fn found_bd(st: &mut State, line: &str) {
    st.in_border = true;
    st.bord_pos = if line.len() >= 16 {
        line[6..16].to_string()
    } else {
        String::new()
    };
}

/// Port of `CombineBorders.combine_borders`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file.
pub fn combine_borders(content: &str) -> String {
    let mut st = State {
        in_border: false,
        bord_pos: String::new(),
        bord_att: Vec::new(),
    };
    let mut out = String::new();

    for line in content.lines() {
        let five = first_five(line);
        if st.in_border {
            // Port of `__border_func`.
            if five != "cw<bt" {
                // Port of `end_border`: flush the collected border
                // attributes as one combined `cw<bd<...` line, then
                // either immediately re-enter border-collection for a
                // fresh `cw<bd<...` line (its own opening line is
                // *not* separately written -- it's superseded by the
                // combined line that will close the run once this new
                // border ends), or write the current (non-border)
                // line through untouched.
                let border_string = st.bord_att.join("|");
                st.bord_att.clear();
                out.push_str(&format!("cw<bd<{}<nu<{border_string}\n", st.bord_pos));
                st.in_border = false;
                if five == "cw<bd" {
                    found_bd(&mut st, line);
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            } else {
                add_to_border_desc(&mut st, line);
            }
        } else {
            // Port of `__default_func`.
            if five == "cw<bd" {
                found_bd(&mut st, line);
                // matches `return ''` -- nothing written for this line
                // (its own opening line is dropped, see note above).
            } else {
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

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn non_border_lines_pass_through_unchanged() {
        let content = lines(&["tx<nu<__________<hello", "cw<ci<bold______<nu<true"]);
        assert_eq!(combine_borders(&content), content);
    }

    #[test]
    fn single_border_with_one_attribute_is_combined() {
        let content = lines(&[
            "cw<bd<bor-t-r-vi<nu<",
            "cw<bt<bdr-single<nu<true",
            "tx<nu<__________<next",
        ]);
        let out = combine_borders(&content);
        assert_eq!(
            out,
            lines(&["cw<bd<bor-t-r-vi<nu<bdr-single", "tx<nu<__________<next"])
        );
    }

    #[test]
    fn border_with_multiple_attributes_joined_with_pipe() {
        let content = lines(&[
            "cw<bd<bor-par-le<nu<",
            "cw<bt<bdr-single<nu<true",
            "cw<bt<bdr-li-wid<nu<0.50",
            &cb_end(),
        ]);
        let out = combine_borders(&content);
        assert_eq!(
            out,
            lines(&["cw<bd<bor-par-le<nu<bdr-single|bdr-li-wid:0.50", &cb_end()])
        );
    }

    fn cb_end() -> String {
        "cb<nu<clos-brack<0001".to_string()
    }

    #[test]
    fn non_true_attribute_value_is_kept_with_colon_prefix() {
        let content = lines(&[
            "cw<bd<bor-par-to<nu<",
            "cw<bt<bdr-color_<nu<FF",
            "tx<nu<__________<next",
        ]);
        let out = combine_borders(&content);
        assert_eq!(
            out,
            lines(&["cw<bd<bor-par-to<nu<bdr-color_:FF", "tx<nu<__________<next"])
        );
    }

    /// A border run left dangling at end-of-file (never followed by a
    /// non-`cw<bt` line to trigger `end_border`) is never flushed --
    /// matches the Python, whose loop simply ends while `__state ==
    /// 'border'`, leaving `__bord_att` unflushed.
    #[test]
    fn dangling_border_at_eof_is_never_flushed() {
        let content = lines(&["cw<bd<bor-par-to<nu<", "cw<bt<bdr-color_<nu<FF"]);
        let out = combine_borders(&content);
        assert_eq!(out, "");
    }

    #[test]
    fn back_to_back_borders_each_produce_their_own_combined_line() {
        let content = lines(&[
            "cw<bd<bor-par-le<nu<",
            "cw<bt<bdr-single<nu<true",
            "cw<bd<bor-par-ri<nu<",
            "cw<bt<bdr-dashed<nu<true",
            "tx<nu<__________<after",
        ]);
        let out = combine_borders(&content);
        assert_eq!(
            out,
            lines(&[
                "cw<bd<bor-par-le<nu<bdr-single",
                "cw<bd<bor-par-ri<nu<bdr-dashed",
                "tx<nu<__________<after",
            ])
        );
    }

    #[test]
    fn border_with_no_bt_lines_produces_empty_attribute_string() {
        // e.g. `cw<bd<bor-none__<nu<false` type lines are ordinary
        // (non-`bd`) cw lines and wouldn't enter border state at all,
        // but a `cw<bd` line immediately followed by an unrelated line
        // still needs to flush with an empty joined string.
        let content = lines(&["cw<bd<bor-par-bo<nu<", "tx<nu<__________<immediately after"]);
        let out = combine_borders(&content);
        assert_eq!(
            out,
            lines(&["cw<bd<bor-par-bo<nu<", "tx<nu<__________<immediately after"])
        );
    }
}
