//! Port of `old_src/src/calibre/ebooks/rtf2xml/fields_large.py`
//! (`FieldsLarge`).
//!
//! Wraps Word's `\field{\*\fldinst ...}{\fldrslt ...}` fields --
//! everything except toc/index-entry fields and bookmarks (see
//! [`super::fields_small`]) -- in `<field>`/`<field-block>` tags.
//! Fields can nest; a `Vec<FieldFrame>` stack (mirroring Python's four
//! parallel lists indexed by `[-1]`) tracks each nesting level's own
//! bracket count, accumulated body text, field-instruction attribute
//! string, and whether a paragraph or section break occurred inside
//! it.
//!
//! Delegates the field-instruction text itself to
//! [`super::field_strings::process_string`] (a separate, earlier PR).
//! The `symbol` flag Python tracks (`self.__symbol`, set when a
//! `SYMBOL` field-instruction resolves) is a single flat `bool` here
//! too, not per-frame -- true to the original: it's read and reset by
//! the very next [`end_field`] call after being set, and RTF's strict
//! bracket nesting guarantees that's always the field whose
//! instruction just set it.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files -- the temp-file / [`super::copy`] / rename dance around the
//! real pass is pipeline plumbing, not ported here.

use super::field_strings::{self, FieldInstruction};

const MARKER: &str = "mi<mk<inline-fld\n";

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 { line[line.len() - 4..].to_string() } else { line.to_string() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeBody,
    InBody,
    Field,
    FieldInstruction,
}

/// One nesting level's worth of Python's four parallel
/// `self.__field_count` / `self.__field_string` / `self.__sec_in_field`
/// / `self.__par_in_field` lists, plus (unlike Python, which pops this
/// from a fifth, separately-tracked `self.__field_instruction` list)
/// its own field-instruction attribute string, to keep it tied to the
/// frame it belongs to rather than relying on stack-depth parity
/// between two lists.
struct FieldFrame {
    bracket_count: String,
    text: String,
    sec_in_field: bool,
    par_in_field: bool,
    instruction: String,
}

/// Port of `__end_field_func`. Pops `frames`' top frame, builds its
/// wrapped `<field>`/`<field-block>` text, and either appends it into
/// the new top frame (this was a nested field) or writes it straight
/// to `out` (this was the outermost field) -- returning `true` in the
/// latter case, so the caller knows to go back to [`State::InBody`].
fn end_field(out: &mut String, frames: &mut Vec<FieldFrame>, symbol: &mut bool) -> bool {
    let frame = frames.pop().expect("end_field only called with a frame on the stack");
    let last_bracket = frame.bracket_count;
    let instruction = frame.instruction;

    let mut inner = if *symbol {
        // The accumulated body text is discarded entirely -- a Symbol
        // field is really UTF-8 data, not a field, so its "body" is
        // replaced by the instruction handler's own synthesized
        // `tx<hx<...` line plus a closing bracket reusing the field's
        // own opening bracket number.
        format!("{instruction}cb<nu<clos-brack<{last_bracket}\n")
    } else if frame.sec_in_field || frame.par_in_field {
        format!(
            "mi<mk<fldbkstart\nmi<tg<open-att__<field-block<type>{instruction}\n{}mi<mk<fldbk-end_\nmi<tg<close_____<field-block\nmi<mk<fld-bk-end\n",
            frame.text
        )
    } else {
        format!(
            "{MARKER}mi<tg<open-att__<field<type>{instruction}\n{}mi<tg<close_____<field\n",
            frame.text
        )
    };
    if frame.sec_in_field {
        inner = format!("mi<mk<sec-fd-beg\n{inner}mi<mk<sec-fd-end\n");
    }
    if frame.par_in_field {
        inner = format!("mi<mk<par-in-fld\n{inner}");
    }
    *symbol = false;

    match frames.last_mut() {
        Some(parent) => {
            parent.text.push_str(&inner);
            false
        }
        None => {
            out.push_str(&inner);
            true
        }
    }
}

/// Port of `FieldsLarge.fix_fields`, operating directly on
/// intermediate-format content (see this module's own docs) rather
/// than reopening a file. Propagates
/// [`field_strings::process_string`]'s error (an unrecognized field
/// name at `run_level > 3`) rather than swallowing it.
pub fn fix_fields(content: &str, run_level: u32) -> field_strings::Result<String> {
    let mut state = State::BeforeBody;
    let mut out = String::new();

    let mut ob_count = String::new();
    let mut cb_count = String::new();

    let mut field_instruction_count = String::new();
    let mut field_instruction_string = String::new();
    let mut symbol = false;

    let mut frames: Vec<FieldFrame> = Vec::new();

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
                if tok == "mi<mk<body-open_" {
                    state = State::InBody;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::InBody => {
                if tok == "cw<fd<field_____" {
                    state = State::Field;
                    cb_count.clear();
                    frames.push(FieldFrame {
                        bracket_count: ob_count.clone(),
                        text: String::new(),
                        sec_in_field: false,
                        par_in_field: false,
                        instruction: String::new(),
                    });
                }
                out.push_str(line);
                out.push('\n');
            }
            State::Field => {
                let top_bracket = frames.last().expect("State::Field implies a frame").bracket_count.clone();
                if cb_count == top_bracket {
                    let frame = frames.last_mut().unwrap();
                    frame.text.push_str(line);
                    frame.text.push('\n');
                    if end_field(&mut out, &mut frames, &mut symbol) {
                        state = State::InBody;
                    }
                } else {
                    match tok {
                        "cw<fd<field-inst" => {
                            state = State::FieldInstruction;
                            field_instruction_count = ob_count.clone();
                            cb_count.clear();
                        }
                        "cw<fd<field_____" => {
                            cb_count.clear();
                            frames.push(FieldFrame {
                                bracket_count: ob_count.clone(),
                                text: String::new(),
                                sec_in_field: false,
                                par_in_field: false,
                                instruction: String::new(),
                            });
                        }
                        "cw<pf<par-end___" => {
                            let frame = frames.last_mut().unwrap();
                            frame.text.push_str(line);
                            frame.text.push('\n');
                            frame.par_in_field = true;
                        }
                        "cw<sc<section___" => {
                            let frame = frames.last_mut().unwrap();
                            frame.text.push_str(line);
                            frame.text.push('\n');
                            frame.sec_in_field = true;
                        }
                        _ => {
                            let frame = frames.last_mut().unwrap();
                            frame.text.push_str(line);
                            frame.text.push('\n');
                        }
                    }
                }
            }
            State::FieldInstruction => {
                if cb_count == field_instruction_count {
                    let frame = frames.last_mut().unwrap();
                    frame.text.push_str(line);
                    frame.text.push('\n');

                    let FieldInstruction { is_symbol, content: instruction } =
                        field_strings::process_string(&field_instruction_string, run_level)?;
                    frame.instruction = instruction;
                    if is_symbol {
                        symbol = true;
                    }
                    state = State::Field;
                    field_instruction_string.clear();
                } else {
                    field_instruction_string.push_str(line);
                    field_instruction_string.push('\n');
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_body(inner: &str) -> String {
        format!("mi<mk<body-open_\n{inner}")
    }

    #[test]
    fn content_before_the_body_is_passed_through_unchanged() {
        let content = "ig<nu<__________<preamble\nmi<mk<body-open_\n";
        assert_eq!(fix_fields(content, 1).unwrap(), content);
    }

    #[test]
    fn plain_body_lines_pass_through_unchanged() {
        let content = wrap_body("tx<nu<__________<hello world\n");
        assert_eq!(fix_fields(&content, 1).unwrap(), content);
    }

    #[test]
    fn wraps_a_simple_field_in_field_tags() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<PAGE \n\
             cb<nu<clos-brack<0002\n\
             tx<nu<__________<5\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content, 1).unwrap();
        assert!(out.contains(MARKER), "{out}");
        assert!(
            out.contains("mi<tg<open-att__<field<type>insert-page-number<update>static\n"),
            "{out}"
        );
        assert!(out.contains("mi<tg<close_____<field\n"), "{out}");
        assert!(out.contains("tx<nu<__________<5\n"), "{out}");
        // The field-instruction's own sub-group brackets are consumed,
        // not left dangling in the output.
        assert!(!out.contains("cw<fd<field-inst"), "{out}");
    }

    #[test]
    fn a_paragraph_break_inside_a_field_produces_a_field_block_wrapped_in_par_in_fld() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<TOC \n\
             cb<nu<clos-brack<0002\n\
             cw<pf<par-end___<nu<true\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content, 1).unwrap();
        assert!(out.contains("mi<mk<par-in-fld\n"), "{out}");
        assert!(out.contains("mi<tg<open-att__<field-block<type>table-of-contents"), "{out}");
        assert!(out.contains("mi<tg<close_____<field-block\n"), "{out}");
        // A field-block, not a plain field.
        assert!(!out.contains("mi<tg<open-att__<field<type>"), "{out}");
    }

    #[test]
    fn a_section_break_inside_a_field_wraps_with_sec_fd_markers() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<TOC \n\
             cb<nu<clos-brack<0002\n\
             cw<sc<section___<nu<true\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content, 1).unwrap();
        assert!(out.contains("mi<mk<sec-fd-beg\n"), "{out}");
        assert!(out.contains("mi<mk<sec-fd-end\n"), "{out}");
    }

    #[test]
    fn a_symbol_field_instruction_replaces_the_field_body_with_synthesized_utf8() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<SYMBOL 97 \\f \"Symbol\" \\s 12\n\
             cb<nu<clos-brack<0002\n\
             tx<nu<__________<ignored fldrslt text\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content, 1).unwrap();
        assert!(out.contains("tx<hx<__________<'61\n"), "{out}");
        assert!(!out.contains("ignored fldrslt text"), "{out}");
        // No <field> wrapper at all for a Symbol field.
        assert!(!out.contains(MARKER), "{out}");
    }

    #[test]
    fn a_nested_field_is_embedded_inside_its_parents_field_tag() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<TOC \n\
             cb<nu<clos-brack<0002\n\
             ob<nu<open-brack<0003\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0004\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<PAGEREF _Toc1 \\h\n\
             cb<nu<clos-brack<0004\n\
             tx<nu<__________<1\n\
             cb<nu<clos-brack<0003\n\
             cb<nu<clos-brack<0001\n",
        );
        let out = fix_fields(&content, 1).unwrap();
        // Both the outer TOC field and the inner PAGEREF field
        // produced their own <field> tags, and the inner one's marker
        // ended up nested inside the outer field's content.
        assert_eq!(out.matches(MARKER).count(), 2, "{out}");
        let outer_open = out.find("type>table-of-contents").unwrap();
        let inner_marker = out.rfind(MARKER).unwrap();
        let outer_close = out.rfind("mi<tg<close_____<field\n").unwrap();
        assert!(outer_open < inner_marker && inner_marker < outer_close, "{out}");
    }

    #[test]
    fn unrecognized_field_name_errors_at_high_run_level() {
        let content = wrap_body(
            "ob<nu<open-brack<0001\n\
             cw<fd<field_____<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<fd<field-inst<nu<true\n\
             tx<nu<__________<NOTAREALFIELD\n\
             cb<nu<clos-brack<0002\n\
             cb<nu<clos-brack<0001\n",
        );
        assert!(fix_fields(&content, 4).is_err());
        assert!(fix_fields(&content, 1).is_ok());
    }
}
