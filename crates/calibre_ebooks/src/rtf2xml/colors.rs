//! Port of `old_src/src/calibre/ebooks/rtf2xml/colors.py` (`Colors`).
//!
//! Runs after [`super::fonts`], before `styles.py`, on the
//! bracket-tagged intermediate format described in
//! [`super::process_tokens`]'s module docs. Reads the color table
//! (`mi<mk<clrtbl-beg` .. `mi<mk<clrtbl-end`, one `cw<ci<red_______` /
//! `cw<ci<green_____` / `cw<ci<blue______` triple per color) once,
//! building a color-number -> `#RRGGBB` map and emitting a
//! `color-in-table` tag per color found; then, for the remainder of the
//! document, rewrites `cw<ci<font-color<nu<{num}` lines and any
//! `bdr-color_:{num}` substrings inside border (`cw<bd<...`) lines to
//! use the resolved hex value instead of the table number.

use indexmap::IndexMap;
use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;

/// Errors [`convert_colors`] can return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ColorsError {
    /// Port of `__in_color_func`'s `action = self.__state_dict.get(...)`
    /// followed unconditionally by `action(line)`: when no action
    /// matches a token seen inside the color table, the Python writes
    /// a diagnostic to stderr and then crashes with an uncaught
    /// `TypeError` (calling `None(line)`). There is no run_level gate
    /// on this path at all -- preserved here as a hard error rather
    /// than silently ignoring the unrecognized token, mirroring the
    /// Python's own inability to continue past it.
    #[error("in module colors.py\nfunction is self.__in_color_func\nno action for {0}")]
    NoActionForToken(String),
    /// Port of `__figure_num`'s `run_level > 3` gated
    /// `raise self.__bug_handler(msg)`. Below that threshold the
    /// Python silently degrades to the string `"0"` instead -- see
    /// [`figure_num`]'s doc comment.
    #[error("no value in self.__color_dict for key {num} at line {line}\n")]
    MissingColorTableEntry { num: i64, line: usize },
    /// Port of `__sub_from_line_color`'s `run_level > 3` gated
    /// `raise self.__bug_handler("can't make integer from string\n")`,
    /// reached on `int(num)` raising `ValueError`. The Python's own
    /// regex (`bdr-color_:(\d+)`) only ever captures digit runs, so
    /// this specifically requires an integer *too large* to parse (a
    /// dead branch for any realistic border-color number, kept and
    /// tested via a deliberately oversized digit run -- see the
    /// `oversized_bdr_color_number_hits_the_dead_valueerror_branch`
    /// test below).
    #[error("can't make integer from string\n")]
    CannotParseColorNumber,
    /// Port of `__after_color_func`'s unguarded `int(line[20:-1])` for
    /// a `cw<ci<font-color` line -- the Python has no `try`/`except`
    /// here at all, so a non-numeric value crashes unconditionally
    /// (regardless of `run_level`), exactly as [`NoActionForToken`]
    /// does for the color-table dispatch.
    ///
    /// [`NoActionForToken`]: ColorsError::NoActionForToken
    #[error("invalid non-numeric font-color value {0:?}")]
    InvalidFontColorNumber(String),
}

pub type Result<T> = std::result::Result<T, ColorsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // all three states are genuinely about the color table
enum State {
    BeforeColorTable,
    InColorTable,
    AfterColorTable,
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

/// Port of `line[-3:-1]` (a Python line with its trailing `\n` still
/// attached) applied to a `str::lines()` line (trailing `\n` already
/// stripped): the last 2 characters -- the 2-digit hex channel value.
fn last_two(line: &str) -> &str {
    let len = line.len();
    line.get(len.saturating_sub(2)..).unwrap_or(line)
}

/// Byte-offset slice tolerant of lines shorter than `idx`, mirroring
/// Python's forgiving slice semantics.
fn slice_from(line: &str, idx: usize) -> &str {
    line.get(idx..).unwrap_or("")
}

/// Port of `__figure_num`. `num == 0` always means "no color" (`false`,
/// a real RTF convention, not a boolean literal); otherwise looks up
/// `num` in the color table built during the color-table pass. When
/// absent, degrades to the literal string `"0"` below `run_level > 3`
/// (mirroring [`super::process_tokens`]'s `divide_num` -- see that
/// function's own doc for the general shape of this quirk) rather than
/// erroring.
fn figure_num(
    color_dict: &IndexMap<i64, String>,
    num: i64,
    run_level: u32,
    line_num: usize,
) -> Result<String> {
    let hex_num = if num == 0 {
        Some("false".to_string())
    } else {
        color_dict.get(&num).cloned()
    };
    match hex_num {
        Some(v) => Ok(v),
        None => {
            if run_level > 3 {
                Err(ColorsError::MissingColorTableEntry {
                    num,
                    line: line_num,
                })
            } else {
                Ok("0".to_string())
            }
        }
    }
}

/// Port of `__sub_from_line_color`, applied manually (rather than via
/// `regex::Regex::replace_all`'s closure, which cannot propagate a
/// `Result`) to every `bdr-color_:(\d+)` match in a `cw<bd<...` line.
fn substitute_bdr_colors(
    line: &str,
    color_dict: &IndexMap<i64, String>,
    run_level: u32,
    line_num: usize,
) -> Result<String> {
    lazy_static! {
        static ref LINE_COLOR_EXP: Regex = Regex::new(r"bdr-color_:(\d+)").unwrap();
    }
    let mut result = String::with_capacity(line.len());
    let mut last_end = 0;
    for caps in LINE_COLOR_EXP.captures_iter(line) {
        let whole = caps.get(0).unwrap();
        result.push_str(&line[last_end..whole.start()]);
        let num_str = &caps[1];
        let replacement = match num_str.parse::<i64>() {
            Ok(num) => {
                let hex_num = figure_num(color_dict, num, run_level, line_num)?;
                format!("bdr-color_:{hex_num}")
            }
            Err(_) => {
                if run_level > 3 {
                    return Err(ColorsError::CannotParseColorNumber);
                }
                "bdr-color_:no-value".to_string()
            }
        };
        result.push_str(&replacement);
        last_end = whole.end();
    }
    result.push_str(&line[last_end..]);
    Ok(result)
}

/// Port of `__after_color_func`.
fn after_color_func(
    line: &str,
    tok: &str,
    color_dict: &IndexMap<i64, String>,
    run_level: u32,
    line_num: usize,
    out: &mut String,
) -> Result<()> {
    if tok == "cw<ci<font-color" {
        let num_str = slice_from(line, 20);
        let num: i64 = num_str
            .parse()
            .map_err(|_| ColorsError::InvalidFontColorNumber(num_str.to_string()))?;
        let hex_num = figure_num(color_dict, num, run_level, line_num)?;
        // Real upstream quirk: Python's `if hex_num:` is *always* true
        // here -- `figure_num` never returns `None` or an empty
        // string (it returns `"false"`, a real hex value, or the `"0"`
        // degrade string) -- so this write is unconditional in
        // practice, matching the always-taken branch verbatim.
        out.push_str(&format!("cw<ci<font-color<nu<{hex_num}\n"));
    } else if line.starts_with("cw<bd") {
        if line.contains("bdr-color_") {
            let substituted = substitute_bdr_colors(line, color_dict, run_level, line_num)?;
            out.push_str(&substituted);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    } else {
        out.push_str(line);
        out.push('\n');
    }
    Ok(())
}

/// Port of `Colors.convert_colors` (the temp-file / `Copy` / rename
/// dance is pipeline plumbing, not ported here -- see
/// [`super::process_tokens::process_tokens`]'s own doc for the same
/// call).
pub fn convert_colors(content: &str, run_level: u32) -> Result<String> {
    let mut state = State::BeforeColorTable;
    let mut out = String::new();
    let mut color_dict: IndexMap<i64, String> = IndexMap::new();
    let mut color_string = String::from("#");
    let mut color_num: i64 = 1;

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let tok = token_info(line);
        match state {
            State::BeforeColorTable => {
                // Port of `__before_color_func`.
                if tok == "mi<mk<clrtbl-beg" {
                    state = State::InColorTable;
                }
                out.push_str(line);
                out.push('\n');
            }
            State::InColorTable => {
                // Port of `__in_color_func`.
                if tok == "mi<mk<clrtbl-end" {
                    state = State::AfterColorTable;
                } else {
                    match tok {
                        "cw<ci<red_______" | "cw<ci<green_____" => {
                            // Port of `__default_color_func`.
                            color_string.push_str(last_two(line));
                        }
                        "cw<ci<blue______" => {
                            // Port of `__blue_func`.
                            color_string.push_str(last_two(line));
                            color_dict.insert(color_num, color_string.clone());
                            out.push_str(&format!(
                                "mi<tg<empty-att_<color-in-table<num>{color_num}<value>{color_string}\n"
                            ));
                            color_num += 1;
                            color_string = "#".to_string();
                        }
                        "tx<nu<__________" => {
                            // Port of `__do_nothing_func`: bad RTF (or
                            // the color table's own leading bare `;`
                            // for color index 0/auto) puts plain text
                            // in the table -- dropped, not written.
                        }
                        other => {
                            eprintln!(
                                "in module colors.py\nfunction is self.__in_color_func\nno action for {other}"
                            );
                            return Err(ColorsError::NoActionForToken(other.to_string()));
                        }
                    }
                }
            }
            State::AfterColorTable => {
                after_color_func(line, tok, &color_dict, run_level, line_num, &mut out)?;
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_before_color_table() {
        let input = "some-line\nanother\n";
        let out = convert_colors(input, 1).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn color_table_builds_dict_and_emits_tags() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "tx<nu<__________<;\n", // leading bare ';' -> color index 0/auto, dropped
            "cw<ci<red_______<nu<FF\n",
            "cw<ci<green_____<nu<00\n",
            "cw<ci<blue______<nu<00\n",
            "mi<mk<clrtbl-end\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("mi<tg<empty-att_<color-in-table<num>1<value>#FF0000\n"));
        assert!(!out.contains("tx<nu<__________<;"));
    }

    #[test]
    fn font_color_zero_resolves_to_false() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<ci<font-color<nu<0\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("cw<ci<font-color<nu<false\n"));
    }

    #[test]
    fn font_color_resolves_to_hex_from_table() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "cw<ci<red_______<nu<AA\n",
            "cw<ci<green_____<nu<BB\n",
            "cw<ci<blue______<nu<CC\n",
            "mi<mk<clrtbl-end\n",
            "cw<ci<font-color<nu<1\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("cw<ci<font-color<nu<#AABBCC\n"));
    }

    #[test]
    fn missing_color_degrades_to_zero_at_low_run_level() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<ci<font-color<nu<9\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("cw<ci<font-color<nu<0\n"));
    }

    #[test]
    fn missing_color_errors_above_run_level_three() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<ci<font-color<nu<9\n",
        );
        let err = convert_colors(input, 4).unwrap_err();
        assert_eq!(err, ColorsError::MissingColorTableEntry { num: 9, line: 3 });
    }

    #[test]
    fn border_color_substring_is_resolved_in_place() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "cw<ci<red_______<nu<11\n",
            "cw<ci<green_____<nu<22\n",
            "cw<ci<blue______<nu<33\n",
            "mi<mk<clrtbl-end\n",
            "cw<bd<bor-par-to<nu<bdr-hair__|bdr-li-wid:0.50|bdr-color_:1\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("bdr-color_:#112233\n"));
    }

    #[test]
    fn border_line_without_bdr_color_passes_through_unchanged() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<bd<bor-par-to<nu<bdr-hair__|bdr-li-wid:0.50\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("cw<bd<bor-par-to<nu<bdr-hair__|bdr-li-wid:0.50\n"));
    }

    #[test]
    fn unrecognized_token_in_color_table_errors() {
        let input = concat!("mi<mk<clrtbl-beg\n", "cw<ci<unknown____<nu<1\n",);
        let err = convert_colors(input, 1).unwrap_err();
        assert!(matches!(err, ColorsError::NoActionForToken(_)));
    }

    #[test]
    fn oversized_bdr_color_number_hits_the_dead_valueerror_branch() {
        // 20 nines overflows i64 (max ~9.2e18), the only realistic way
        // to make the regex-guaranteed-digits capture fail to parse.
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<bd<bor-par-to<nu<bdr-color_:99999999999999999999\n",
        );
        let out = convert_colors(input, 1).unwrap();
        assert!(out.contains("bdr-color_:no-value\n"));

        let err = convert_colors(input, 4).unwrap_err();
        assert_eq!(err, ColorsError::CannotParseColorNumber);
    }

    #[test]
    fn non_numeric_font_color_value_errors() {
        let input = concat!(
            "mi<mk<clrtbl-beg\n",
            "mi<mk<clrtbl-end\n",
            "cw<ci<font-color<nu<x\n",
        );
        let err = convert_colors(input, 1).unwrap_err();
        assert_eq!(err, ColorsError::InvalidFontColorNumber("x".to_string()));
    }
}
