//! Port of `old_src/src/calibre/ebooks/rtf2xml/fonts.py` (`Fonts`).
//!
//! Runs after `hex_2_utf8`'s preamble pass, before [`super::colors`], on
//! the bracket-tagged intermediate format described in
//! [`super::process_tokens`]'s module docs. Reads the font table
//! (`mi<mk<fonttb-beg` .. `mi<mk<fonttb-end`, with individual fonts
//! delimited by `mi<mk<fontit-beg` .. `mi<mk<fontit-end`) once, building
//! a font-number -> font-name map and emitting an `empty-att_` tag per
//! font found; then, for the remainder of the document, rewrites every
//! `cw<ci<font-style<nu<{num}` line to use the resolved font *name*
//! instead of its table number.

use indexmap::IndexMap;
use thiserror::Error;

/// Errors [`convert_fonts`] can return. Mirrors the Python's
/// `run_level > 3`-gated `raise self.__bug_handler(msg)` in
/// `__after_font_table_func`; below that threshold the Python silently
/// degrades instead -- see that function's doc comment below for the
/// precise (surprising) degrade behavior.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FontsError {
    /// Port of `f'no value for {font_num} in self.__font_table\n'`.
    #[error("no value for {0} in self.__font_table\n")]
    MissingFontTableEntry(String),
}

pub type Result<T> = std::result::Result<T, FontsError>;

/// Port of `Fonts.__initiate_values`'s `self.__special_font_dict` +
/// its `'default-font'` entry added at the end of `convert_fonts`.
/// Represented as a struct rather than a generic map (per this crate's
/// porting guide: use strong types where the keys are known) --
/// `symbol`/`wingdings`/`zapf_dingbats` mirror the dict's `0`/`1`
/// integer flags as booleans, `default_font` mirrors its one
/// string-valued entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpecialFontDict {
    pub symbol: bool,
    pub wingdings: bool,
    pub zapf_dingbats: bool,
    pub default_font: String,
}

/// Result of [`convert_fonts`]: the rewritten intermediate-format text
/// plus the special-font dict -- the Python's `convert_fonts` returns
/// only the dict (the file itself is rewritten in place), but nothing
/// here reopens files, so the transformed content is returned
/// alongside it (see [`super::check_brackets`]'s module docs for this
/// crate's `&str` in / owned-value(s) out convention).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontsOutput {
    pub content: String,
    pub special_font_dict: SpecialFontDict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Default,
    FontTable,
    AfterFontTable,
    FontInTable,
}

/// Port of `line[:16]` (the Python's per-line token-type check),
/// tolerant of shorter lines exactly like [`super::check_brackets`]'s
/// own helper of the same shape.
fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Byte-offset slice tolerant of lines shorter than `idx`, mirroring
/// Python's forgiving slice semantics (`line[idx:]` never raises, even
/// on a too-short string) which `&str` indexing does not offer for
/// free.
fn slice_from(line: &str, idx: usize) -> &str {
    line.get(idx..).unwrap_or("")
}

fn mark_special(special: &mut SpecialFontDict, font_name: &str) {
    match font_name {
        "Symbol" => special.symbol = true,
        "Wingdings" => special.wingdings = true,
        "Zapf Dingbats" => special.zapf_dingbats = true,
        _ => {}
    }
}

/// Port of `Fonts.convert_fonts` (the temp-file / `Copy` / rename dance
/// is pipeline plumbing, not ported here -- see
/// [`super::process_tokens::process_tokens`]'s own doc for the same
/// call). `default_font_num` mirrors the constructor's required
/// `default_font_num` argument.
pub fn convert_fonts(content: &str, default_font_num: &str, run_level: u32) -> Result<FontsOutput> {
    let mut state = State::Default;
    let mut out = String::new();
    let mut font_table: IndexMap<String, String> = IndexMap::new();
    let mut font_num = default_font_num.to_string();
    let mut text_line = String::new();
    let mut wrote_ind_font = false;
    let mut special = SpecialFontDict::default();

    for line in content.lines() {
        let tok = token_info(line);
        match state {
            State::Default => {
                // Port of `__default_func`.
                if tok == "mi<mk<fonttb-beg" {
                    state = State::FontTable;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::FontTable => {
                // Port of `__font_table_func`. Note: unlike every other
                // state's handler, this one never writes `line` --
                // matching the Python's commented-out
                // `# self.__write_obj.write(line)`.
                if tok == "mi<mk<fonttb-end" {
                    state = State::AfterFontTable;
                } else if tok == "mi<mk<fontit-beg" {
                    state = State::FontInTable;
                    font_num = default_font_num.to_string();
                    text_line.clear();
                }
            }
            State::FontInTable => {
                // Port of `__font_in_table_func`.
                if tok == "mi<mk<fontit-end" {
                    wrote_ind_font = true;
                    state = State::FontTable;
                    // `self.__text_line[:-1]` -- drop the trailing ';'.
                    text_line.pop();
                    font_table.insert(font_num.clone(), text_line.clone());
                    out.push_str(&format!(
                        "mi<tg<empty-att_<font-in-table<name>{text_line}<num>{font_num}\n"
                    ));
                } else if tok == "cw<ci<font-style" {
                    font_num = slice_from(line, 20).to_string();
                } else if tok == "tx<nu<__________" || tok == "tx<ut<__________" {
                    text_line.push_str(slice_from(line, 17));
                } else if tok == "mi<mk<fonttb-end" {
                    // Port of `__found_end_font_table_func`: a
                    // "premature end" -- the font table closed while
                    // still inside an individual font entry. If no
                    // font was ever fully written, synthesize a
                    // fallback Times/0 entry so downstream passes
                    // always have at least one font-in-table tag.
                    if !wrote_ind_font {
                        out.push_str("mi<tg<empty-att_<font-in-table<name>Times<num>0\n");
                    }
                    state = State::AfterFontTable;
                }
                // Every other line in this state is dropped (matches
                // the Python: no `else` branch writes anything).
            }
            State::AfterFontTable => {
                // Port of `__after_font_table_func`.
                if tok == "cw<ci<font-style" {
                    let num = slice_from(line, 20).to_string();
                    match font_table.get(&num) {
                        Some(font_name) => {
                            let font_name = font_name.clone();
                            mark_special(&mut special, &font_name);
                            out.push_str(&format!("cw<ci<font-style<nu<{font_name}\n"));
                        }
                        None => {
                            if run_level > 3 {
                                return Err(FontsError::MissingFontTableEntry(num));
                            }
                            // Real upstream quirk: below run_level > 3
                            // the Python's `if font_name is None:`
                            // branch has *no* `else` fallback write --
                            // the line is silently dropped entirely,
                            // not passed through and not replaced with
                            // any placeholder. Preserved verbatim; see
                            // the `unmatched_font_number_is_dropped_at_low_run_level`
                            // test below.
                        }
                    }
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }

    let default_font_name = font_table
        .get(default_font_num)
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "Not Defined".to_string());
    special.default_font = default_font_name;

    Ok(FontsOutput {
        content: out,
        special_font_dict: special,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_before_font_table() {
        let input = "some-line\nanother\n";
        let out = convert_fonts(input, "0", 1).unwrap().content;
        assert_eq!(out, input);
    }

    #[test]
    fn single_font_table_entry_resolves_number_to_name() {
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fontit-beg\n",
            "cw<ci<font-style<nu<0\n",
            "tx<nu<__________<Times New Roman;\n",
            "mi<mk<fontit-end\n",
            "mi<mk<fonttb-end\n",
            "cw<ci<font-style<nu<0\n",
        );
        let result = convert_fonts(input, "0", 1).unwrap();
        assert!(result
            .content
            .contains("mi<tg<empty-att_<font-in-table<name>Times New Roman<num>0\n"));
        assert!(result
            .content
            .contains("cw<ci<font-style<nu<Times New Roman\n"));
        assert_eq!(result.special_font_dict.default_font, "Times New Roman");
    }

    #[test]
    fn font_num_defaults_when_no_font_style_line_precedes_text() {
        // A font entry that never sees a `cw<ci<font-style` line before
        // its text keeps the constructor's `default_font_num`.
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fontit-beg\n",
            "tx<nu<__________<Arial;\n",
            "mi<mk<fontit-end\n",
            "mi<mk<fonttb-end\n",
        );
        let result = convert_fonts(input, "2", 1).unwrap();
        assert!(result
            .content
            .contains("mi<tg<empty-att_<font-in-table<name>Arial<num>2\n"));
    }

    #[test]
    fn special_font_names_set_their_flags() {
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fontit-beg\n",
            "cw<ci<font-style<nu<1\n",
            "tx<nu<__________<Symbol;\n",
            "mi<mk<fontit-end\n",
            "mi<mk<fonttb-end\n",
            "cw<ci<font-style<nu<1\n",
        );
        let result = convert_fonts(input, "0", 1).unwrap();
        assert!(result.special_font_dict.symbol);
        assert!(!result.special_font_dict.wingdings);
        assert!(!result.special_font_dict.zapf_dingbats);
    }

    #[test]
    fn unmatched_font_number_is_dropped_at_low_run_level() {
        // Real upstream quirk (see `MissingFontTableEntry`'s doc):
        // below run_level > 3, a `cw<ci<font-style` referring to a
        // number absent from the font table produces *no* output line
        // at all for that token -- not the original, not a fallback.
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fonttb-end\n",
            "cw<ci<font-style<nu<99\n",
            "tx<nu<__________<after\n",
        );
        let result = convert_fonts(input, "0", 1).unwrap();
        assert!(!result.content.contains("font-style"));
        assert!(result.content.contains("tx<nu<__________<after\n"));
    }

    #[test]
    fn unmatched_font_number_errors_above_run_level_three() {
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fonttb-end\n",
            "cw<ci<font-style<nu<99\n",
        );
        let err = convert_fonts(input, "0", 4).unwrap_err();
        assert_eq!(err, FontsError::MissingFontTableEntry("99".to_string()));
    }

    #[test]
    fn premature_font_table_end_writes_fallback_times_entry() {
        // The font table ends (`fonttb-end`) while still inside an
        // individual font entry (no `fontit-end` seen) -- and no font
        // was ever fully written -- so a fallback `Times`/`0` entry is
        // synthesized.
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fontit-beg\n",
            "mi<mk<fonttb-end\n",
        );
        let result = convert_fonts(input, "0", 1).unwrap();
        assert!(result
            .content
            .contains("mi<tg<empty-att_<font-in-table<name>Times<num>0\n"));
    }

    #[test]
    fn no_premature_fallback_once_a_real_font_was_written() {
        let input = concat!(
            "mi<mk<fonttb-beg\n",
            "mi<mk<fontit-beg\n",
            "tx<nu<__________<Arial;\n",
            "mi<mk<fontit-end\n",
            "mi<mk<fontit-beg\n",
            "mi<mk<fonttb-end\n",
        );
        let result = convert_fonts(input, "0", 1).unwrap();
        assert!(!result.content.contains("<name>Times<num>0"));
    }

    #[test]
    fn default_font_name_falls_back_when_default_num_never_seen() {
        let input = concat!("mi<mk<fonttb-beg\n", "mi<mk<fonttb-end\n",);
        let result = convert_fonts(input, "5", 1).unwrap();
        assert_eq!(result.special_font_dict.default_font, "Not Defined");
    }
}
