//! Port of `old_src/src/calibre/ebooks/rtf2xml/paragraphs.py`
//! (`Paragraphs`).
//!
//! Runs after `sections.py`, before `paragraph_def.py`, on the
//! bracket-tagged intermediate format described in
//! [`super::process_tokens`]'s module docs. RTF never marks the *start*
//! of a paragraph, only its end (`\par`), so this pass infers paragraph
//! boundaries: starting in the document body assumed *not* in a
//! paragraph, it opens one as soon as it sees a clue that one has begun
//! (a run of text, an inline field, list-text, a picture), and closes it
//! on a clue that it has ended (`\par`, the end of a footnote/header, a
//! paragraph definition, the end of a field-block, or the start of a
//! section). Has no `bug_handler`-raised error path at all -- like
//! [`super::preamble_rest`], every branch either writes output or
//! changes state, so [`make_paragraphs`] returns a plain `String` rather
//! than a `Result`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeBody,
    NotParagraph,
    Paragraph,
}

const START_MARKER: &str = "mi<mk<para-start\n"; // outside para tags
const START2_MARKER: &str = "mi<mk<par-start_\n"; // inside para tags
const END2_MARKER: &str = "mi<mk<par-end___\n"; // inside para tags
const END_MARKER: &str = "mi<mk<para-end__\n"; // outside para tags

/// Port of `line[:16]`, tolerant of shorter lines (see
/// [`super::check_brackets`]'s helper of the same shape).
fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Port of `__start_para_func`: opens a paragraph and switches state.
/// The triggering line itself is written by the caller afterward (see
/// [`make_paragraphs`]'s `NotParagraph` arm), not here.
fn start_para(out: &mut String, state: &mut State) {
    out.push_str(START_MARKER);
    out.push_str("mi<tg<open______<para\n");
    out.push_str(START2_MARKER);
    *state = State::Paragraph;
}

/// Port of `__empty_para_func`: writes an empty `<para/>` sequence for
/// a `\par` seen outside any paragraph, unless suppressed. State is
/// *not* changed -- the empty paragraph is immediately self-closed, so
/// there is nothing to leave open.
fn empty_para(out: &mut String, write_empty_para: bool) {
    if write_empty_para {
        out.push_str(START_MARKER);
        out.push_str("mi<tg<empty_____<para\n");
        out.push_str(END_MARKER);
    }
}

/// Port of `__empty_pgbk_func`.
fn empty_pgbk(out: &mut String) {
    out.push_str("mi<tg<empty_____<page-break\n");
}

/// Port of `__close_para_func`: closes the open paragraph, writes the
/// triggering `line` itself (as its last step, unlike [`start_para`]),
/// and returns to `NotParagraph`.
fn close_para(out: &mut String, state: &mut State, line: &str) {
    out.push_str(END2_MARKER);
    out.push_str("mi<tg<close_____<para\n");
    out.push_str(END_MARKER);
    out.push_str(line);
    out.push('\n');
    *state = State::NotParagraph;
}

/// Port of `Paragraphs.make_paragraphs` (the temp-file / `Copy` /
/// rename dance is pipeline plumbing, not ported here -- see
/// [`super::process_tokens::process_tokens`]'s own doc for the same
/// call). `write_empty_para` mirrors the constructor's
/// `write_empty_para=1` default.
pub fn make_paragraphs(content: &str, write_empty_para: bool) -> String {
    let mut state = State::BeforeBody;
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        match state {
            State::BeforeBody => {
                // Port of `__before_body_func`.
                if tok == "mi<mk<body-open_" {
                    state = State::NotParagraph;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::NotParagraph => {
                // Port of `__not_paragraph_func`. Real upstream shape
                // worth calling out: whatever the dispatched action
                // does, the *triggering* `line` is then written
                // unconditionally afterward regardless of whether any
                // action matched -- so e.g. a text run that opens a
                // paragraph gets the open-tag markers *and* the text
                // line itself in the output, and a `\par` outside any
                // paragraph gets the empty-para markers (if enabled)
                // *and* the original `\par` token line.
                match tok {
                    "tx<nu<__________" | "tx<hx<__________" | "tx<ut<__________"
                    | "tx<mc<__________" | "mi<mk<inline-fld" | "mi<mk<para-beg__"
                    | "mi<mk<pict-start" => {
                        start_para(&mut out, &mut state);
                    }
                    "cw<pf<par-end___" => {
                        empty_para(&mut out, write_empty_para);
                    }
                    "cw<pf<page-break" => {
                        empty_pgbk(&mut out);
                    }
                    _ => {}
                }
                out.push_str(line);
                out.push('\n');
            }
            State::Paragraph => {
                // Port of `__paragraph_func`. Unlike `NotParagraph`,
                // there is no unconditional trailing write here -- only
                // the `else` (unmatched-token) branch writes `line`;
                // matched actions are responsible for writing it (or
                // not) themselves.
                match tok {
                    "cw<pf<par-end___" | "mi<mk<headi_-end" | "mi<mk<fldbk-end_"
                    | "mi<mk<body-close" | "mi<mk<sect-close" | "mi<mk<sect-start"
                    | "mi<mk<foot___clo" | "cw<tb<cell______" | "mi<mk<par-in-fld" => {
                        close_para(&mut out, &mut state, line);
                    }
                    "cw<pf<par-def___" => {
                        // Port of `__bogus_para__def_func`: a stray
                        // `\pard` inside a paragraph is dropped and
                        // replaced with a marker -- the original line
                        // is *not* written, and (unlike every close
                        // action above) the state does *not* change;
                        // the paragraph stays open.
                        out.push_str("mi<mk<bogus-pard\n");
                    }
                    _ => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_open() -> &'static str {
        "mi<mk<body-open_\n"
    }

    #[test]
    fn before_body_passes_through_and_switches_on_body_open() {
        let input = "abc\nmi<mk<body-open_\n";
        let out = make_paragraphs(input, true);
        assert_eq!(out, input);
    }

    #[test]
    fn text_opens_a_paragraph_and_keeps_the_text_line() {
        let input = format!("{}tx<nu<__________<Hello\n", body_open());
        let out = make_paragraphs(&input, true);
        assert_eq!(
            out,
            concat!(
                "mi<mk<body-open_\n",
                "mi<mk<para-start\n",
                "mi<tg<open______<para\n",
                "mi<mk<par-start_\n",
                "tx<nu<__________<Hello\n",
            )
        );
    }

    #[test]
    fn par_end_closes_an_open_paragraph_and_keeps_the_token_line() {
        let input = format!(
            "{}tx<nu<__________<Hello\ncw<pf<par-end___<nu<true\n",
            body_open()
        );
        let out = make_paragraphs(&input, true);
        assert!(out.contains("mi<mk<par-end___\n"));
        assert!(out.contains("mi<tg<close_____<para\n"));
        assert!(out.contains("mi<mk<para-end__\n"));
        assert!(out.contains("cw<pf<par-end___<nu<true\n"));
    }

    #[test]
    fn empty_paragraph_with_write_empty_para_true_emits_synthetic_and_original() {
        let input = format!("{}cw<pf<par-end___<nu<true\n", body_open());
        let out = make_paragraphs(&input, true);
        assert_eq!(
            out,
            concat!(
                "mi<mk<body-open_\n",
                "mi<mk<para-start\n",
                "mi<tg<empty_____<para\n",
                "mi<mk<para-end__\n",
                "cw<pf<par-end___<nu<true\n",
            )
        );
    }

    #[test]
    fn empty_paragraph_with_write_empty_para_false_suppresses_synthetic_tags() {
        let input = format!("{}cw<pf<par-end___<nu<true\n", body_open());
        let out = make_paragraphs(&input, false);
        assert_eq!(
            out,
            concat!("mi<mk<body-open_\n", "cw<pf<par-end___<nu<true\n",)
        );
    }

    #[test]
    fn page_break_outside_a_paragraph_emits_marker_and_original_line() {
        let input = format!("{}cw<pf<page-break<nu<true\n", body_open());
        let out = make_paragraphs(&input, true);
        assert_eq!(
            out,
            concat!(
                "mi<mk<body-open_\n",
                "mi<tg<empty_____<page-break\n",
                "cw<pf<page-break<nu<true\n",
            )
        );
    }

    #[test]
    fn bogus_pard_inside_a_paragraph_replaces_the_token_and_keeps_the_paragraph_open() {
        let input = format!(
            "{}tx<nu<__________<Hello\ncw<pf<par-def___<nu<true\ntx<nu<__________<World\n",
            body_open()
        );
        let out = make_paragraphs(&input, true);
        assert!(out.contains("mi<mk<bogus-pard\n"));
        assert!(!out.contains("cw<pf<par-def___"));
        // The paragraph never closed: "World" appears as plain
        // passthrough inside the still-open paragraph, with no second
        // open-tag sequence for it.
        assert_eq!(out.matches("mi<tg<open______<para\n").count(), 1);
        assert!(out.contains("tx<nu<__________<World\n"));
    }

    #[test]
    fn section_start_closes_an_open_paragraph() {
        let input = format!("{}tx<nu<__________<Hello\nmi<mk<sect-start\n", body_open());
        let out = make_paragraphs(&input, true);
        assert!(out.contains("mi<tg<close_____<para\n"));
        assert!(out.contains("mi<mk<sect-start\n"));
    }

    #[test]
    fn unmatched_line_in_not_paragraph_state_still_passes_through() {
        let input = format!("{}some-other-token\n", body_open());
        let out = make_paragraphs(&input, true);
        assert_eq!(out, input);
    }

    #[test]
    fn unmatched_line_inside_an_open_paragraph_passes_through() {
        let input = format!(
            "{}tx<nu<__________<Hello\ncw<ci<bold______<nu<true\n",
            body_open()
        );
        let out = make_paragraphs(&input, true);
        assert!(out.contains("cw<ci<bold______<nu<true\n"));
    }
}
