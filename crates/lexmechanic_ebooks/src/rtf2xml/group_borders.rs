//! Port of `old_src/src/calibre/ebooks/rtf2xml/group_borders.py`
//! (`GroupBorders`).
//!
//! Same overall shape as [`super::group_styles`] (runs right after it
//! in the real pipeline -- its style-group boundary markers are among
//! this pass's own `END_LIST` tokens), but groups on a
//! `paragraph-definition`'s own embedded `<border-paragraph...>`
//! attribute substring rather than an external `style-name`: any
//! `paragraph-definition` carrying one gets that substring pulled out
//! into a wrapping `<border-group num="sNNNN">` tag, and consecutive
//! same-border-string paragraphs are always merged into one (there is
//! no `wrap`-style "keep them separate" option here -- see below).
//!
//! # `wrap` is a dead constructor parameter
//!
//! `__init__` takes and stores `self.__wrap`, but unlike
//! [`super::group_styles`] (its obvious template), nothing in this
//! class ever reads it back -- confirmed by grep. Merging
//! same-border-string paragraphs is this pass's *only* behavior;
//! [`group_borders`] therefore takes no `wrap` parameter at all.
//!
//! # A narrow, preserved extraction bug
//!
//! `__parse_pard_with_border`'s regex matches *both*
//! `<border-paragraph...>` and `<border-for-every-paragraph...>`
//! segments, but the code that sorts each matched segment into
//! `border_string` vs `pard_string` only checks a hardcoded 17-char
//! prefix, `token[0:17] == '<border-paragraph'` -- which
//! `<border-for-every-paragraph...>` never matches (its own prefix is
//! `<border-for-every`). So a `<border-for-every-paragraph>` attribute
//! is captured by the split but then silently miscategorized as
//! ordinary paragraph content, landing back in the *plain*
//! `paragraph-definition` tag instead of the border-group wrapper.
//! Preserved literally -- a specific, stable, non-crashing quirk, not
//! a defect that would break the pass's main job.
//!
//! # Confirmed-dead state, omitted
//!
//! `self.__left_indent`, `self.__list_type`, `self.__pard_def`,
//! `self.__all_lists`, `self.__found_appt`, and `self.__line_num` are
//! all assigned once in `__initiate_values` and never read again --
//! confirmed by grep, the same copy-paste-leftover pattern as
//! [`super::make_lists`]/[`super::headings_to_sections`]/
//! [`super::group_styles`]. `__close_pard_` is defined but never
//! called anywhere either (and would crash immediately if it were --
//! it calls a `self.__write_end_wrap()` method that doesn't exist on
//! this class at all, only `__write_end_border_tag` does). None have a
//! Rust counterpart.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;

/// Port of `__after_pard_func`'s `run_level > 2`-gated
/// `raise self.__bug_handler(msg)`, reached when a second
/// `paragraph-definition` close tag is seen with no intervening open
/// (the diagnostic `sys.stderr.write` fires unconditionally before
/// this gate, matching Python).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GroupBordersError {
    #[error("wrong flag")]
    WrongFlag,
}

pub type Result<T> = std::result::Result<T, GroupBordersError>;

lazy_static! {
    static ref NAME_REGEX: Regex = Regex::new(r"<name>[^<]+").unwrap();
    static ref BORDER_REGEX: Regex =
        Regex::new(r"<border-paragraph[^<]+|<border-for-every-paragraph[^<]+").unwrap();
}

const END_LIST: [&str; 17] = [
    "mi<mk<sect-close",
    "mi<mk<sect-start",
    "mi<mk<tabl-start",
    "mi<mk<fldbk-end_",
    "mi<mk<fldbkstart",
    "mi<mk<close_cell",
    "mi<tg<item_end__",
    "mi<mk<foot___clo",
    "mi<mk<footnt-ope",
    "mi<mk<header-beg",
    "mi<mk<header-end",
    "mi<mk<head___clo",
    "mi<mk<list_start",
    "mi<mk<style-grp_",
    "mi<mk<style_grp_",
    "mi<mk<style_gend",
    "mi<mk<stylegend_",
];

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn payload(line: &str) -> &str {
    if line.len() >= 17 { &line[17..] } else { "" }
}

fn is_pard_open(line: &str, tok: &str) -> bool {
    tok == "mi<tg<open-att__" && line.len() >= 37 && &line[17..37] == "paragraph-definition"
}

fn is_pard_close(line: &str, tok: &str) -> bool {
    tok == "mi<tg<close_____" && payload(line) == "paragraph-definition"
}

/// Port of `re.split` with a capturing-group pattern: unlike
/// `Regex::split`, keeps the matched segments interleaved with the
/// non-matched ones, matching Python's own behavior for a pattern
/// whose single capture group spans the whole match (true of
/// [`BORDER_REGEX`]).
fn split_keep_matches(re: &Regex, s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut last_end = 0;
    for m in re.find_iter(s) {
        result.push(s[last_end..m.start()].to_string());
        result.push(m.as_str().to_string());
        last_end = m.end();
    }
    result.push(s[last_end..].to_string());
    result
}

/// Port of `__is_border_func`.
fn is_border(line: &str) -> bool {
    let stripped = NAME_REGEX.replace(line, "");
    stripped.contains("border-paragraph")
}

/// Port of `__parse_pard_with_border` (identical to the unused
/// `__write_pard_with_border`, which is never called -- confirmed by
/// grep -- and has no Rust counterpart).
fn parse_pard_with_border(line: &str) -> (String, String) {
    let mut border_string = String::new();
    let mut pard_string = String::new();
    for token in split_keep_matches(&BORDER_REGEX, line) {
        if token.len() >= 17 && &token[..17] == "<border-paragraph" {
            border_string.push_str(&token);
        } else {
            pard_string.push_str(&token);
        }
    }
    (border_string, pard_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    InPard,
    AfterPard,
}

#[derive(Default)]
struct GroupBordersBuilder {
    state: State,
    list_chunk: String,
    last_border_string: String,
    border_num: u32,
}

impl GroupBordersBuilder {
    fn write_start_border_tag(&mut self, out: &mut String, the_string: &str) {
        out.push_str("mi<mk<start-brdg\n");
        self.border_num += 1;
        let num_string = format!("s{:04}", self.border_num);
        out.push_str(&format!("mi<tg<open-att__<border-group{the_string}<num>{num_string}\n"));
    }

    fn write_end_border_tag(&self, out: &mut String) {
        out.push_str("mi<mk<end-brdg__\nmi<tg<close_____<border-group\n");
    }

    /// Port of `__default_func`.
    fn default_state(&mut self, out: &mut String, line: &str, tok: &str) {
        if is_pard_open(line, tok) && is_border(line) {
            let (border_string, pard_string) = parse_pard_with_border(line);
            self.write_start_border_tag(out, &border_string);
            out.push_str(&pard_string);
            out.push('\n');
            self.last_border_string = border_string;
            self.state = State::InPard;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    /// Port of `__in_pard_func`.
    fn in_pard(&mut self, out: &mut String, line: &str, tok: &str) {
        if is_pard_close(line, tok) {
            self.state = State::AfterPard;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    /// Port of `__pard_after_par_def_func`.
    fn pard_after_par_def(&mut self, out: &mut String, line: &str) {
        if !is_border(line) {
            out.push_str("mi<tg<close_____<paragraph-definition\n");
            self.write_end_border_tag(out);
            out.push_str(&self.list_chunk);
            self.list_chunk.clear();
            out.push_str(line);
            out.push('\n');
            self.state = State::Default;
        } else {
            let (border_string, pard_string) = parse_pard_with_border(line);
            if self.last_border_string == border_string {
                out.push_str("mi<tg<close_____<paragraph-definition\n");
                out.push_str(&self.list_chunk);
                self.list_chunk.clear();
                self.state = State::InPard;
                out.push_str(&pard_string);
                out.push('\n');
            } else {
                out.push_str("mi<tg<close_____<paragraph-definition\n");
                self.write_end_border_tag(out);
                out.push_str(&self.list_chunk);
                self.list_chunk.clear();
                self.write_start_border_tag(out, &border_string);
                out.push_str(&pard_string);
                out.push('\n');
                self.state = State::InPard;
                self.last_border_string = border_string;
            }
        }
    }

    /// Port of `__after_pard_func`.
    fn after_pard(&mut self, out: &mut String, line: &str, tok: &str, run_level: u32) -> Result<()> {
        if is_pard_open(line, tok) {
            self.pard_after_par_def(out, line);
        } else if is_pard_close(line, tok) {
            eprintln!("Wrong flag in __after_pard_func");
            if run_level > 2 {
                return Err(GroupBordersError::WrongFlag);
            }
        } else if END_LIST.contains(&tok) {
            out.push_str("mi<tg<close_____<paragraph-definition\n");
            self.write_end_border_tag(out);
            out.push_str(&self.list_chunk);
            self.list_chunk.clear();
            self.state = State::Default;
            out.push_str(line);
            out.push('\n');
        } else {
            self.list_chunk.push_str(line);
            self.list_chunk.push('\n');
        }
        Ok(())
    }
}

/// Port of `GroupBorders.group_borders`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn group_borders(content: &str, run_level: u32) -> Result<String> {
    let mut b = GroupBordersBuilder::default();
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        match b.state {
            State::Default => b.default_state(&mut out, line, tok),
            State::InPard => b.in_pard(&mut out, line, tok),
            State::AfterPard => b.after_pard(&mut out, line, tok, run_level)?,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pard(attrs: &str, text: &str) -> String {
        format!(
            "mi<tg<open-att__<paragraph-definition{attrs}\ntx<nu<__________<{text}\nmi<tg<close_____<paragraph-definition\n"
        )
    }

    #[test]
    fn plain_content_with_no_paragraph_definitions_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\nmi<mk<tabl-start\n";
        assert_eq!(group_borders(content, 1).unwrap(), content);
    }

    #[test]
    fn a_non_bordered_paragraph_passes_through_completely_untouched() {
        let content = pard("<align>left", "hi");
        assert_eq!(group_borders(&content, 1).unwrap(), content);
    }

    #[test]
    fn a_single_bordered_paragraph_is_wrapped_in_a_numbered_border_group() {
        let mut content = pard("<border-paragraph-bottom>true", "hi");
        content.push_str("mi<mk<tabl-start\n");
        let out = group_borders(&content, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<start-brdg\n\
             mi<tg<open-att__<border-group<border-paragraph-bottom>true<num>s0001\n\
             mi<tg<open-att__<paragraph-definition\n\
             tx<nu<__________<hi\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<end-brdg__\n\
             mi<tg<close_____<border-group\n\
             mi<mk<tabl-start\n"
        );
    }

    #[test]
    fn consecutive_same_border_paragraphs_share_one_border_group() {
        let mut content = pard("<border-paragraph-bottom>true", "one");
        content.push_str(&pard("<border-paragraph-bottom>true", "two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_borders(&content, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<start-brdg\n\
             mi<tg<open-att__<border-group<border-paragraph-bottom>true<num>s0001\n\
             mi<tg<open-att__<paragraph-definition\n\
             tx<nu<__________<one\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<tg<open-att__<paragraph-definition\n\
             tx<nu<__________<two\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<end-brdg__\n\
             mi<tg<close_____<border-group\n\
             mi<mk<tabl-start\n"
        );
    }

    #[test]
    fn consecutive_different_border_paragraphs_get_separate_numbered_groups() {
        let mut content = pard("<border-paragraph-bottom>true", "one");
        content.push_str(&pard("<border-paragraph-top>true", "two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_borders(&content, 1).unwrap();
        assert!(out.contains("<num>s0001"), "{out}");
        assert!(out.contains("<num>s0002"), "{out}");
        assert_eq!(out.matches("mi<tg<open-att__<border-group").count(), 2, "{out}");
        assert_eq!(out.matches("mi<tg<close_____<border-group\n").count(), 2, "{out}");
    }

    #[test]
    fn a_bordered_run_followed_by_a_non_bordered_paragraph_closes_the_group_and_stays_closed() {
        let mut content = pard("<border-paragraph-bottom>true", "one");
        content.push_str(&pard("<align>left", "two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_borders(&content, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<start-brdg\n\
             mi<tg<open-att__<border-group<border-paragraph-bottom>true<num>s0001\n\
             mi<tg<open-att__<paragraph-definition\n\
             tx<nu<__________<one\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<end-brdg__\n\
             mi<tg<close_____<border-group\n\
             mi<tg<open-att__<paragraph-definition<align>left\n\
             tx<nu<__________<two\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<tabl-start\n"
        );
    }

    #[test]
    fn border_for_every_paragraph_is_captured_but_miscategorized_into_the_plain_pard() {
        // See this module's own doc: a genuine, narrow upstream
        // extraction bug, preserved as-is.
        let mut content = pard("<border-paragraph-bottom>true<border-for-every-paragraph>true", "hi");
        content.push_str("mi<mk<tabl-start\n");
        let out = group_borders(&content, 1).unwrap();
        assert!(
            out.contains("mi<tg<open-att__<border-group<border-paragraph-bottom>true<num>s0001\n"),
            "{out}"
        );
        assert!(
            out.contains("mi<tg<open-att__<paragraph-definition<border-for-every-paragraph>true\n"),
            "{out}"
        );
    }

    #[test]
    fn a_double_close_tag_is_a_wrong_flag_error_only_at_high_run_level() {
        let content = "\
mi<tg<open-att__<paragraph-definition<border-paragraph-bottom>true\n\
tx<nu<__________<hi\n\
mi<tg<close_____<paragraph-definition\n\
mi<tg<close_____<paragraph-definition\n";
        assert!(group_borders(content, 2).is_ok());
        assert_eq!(group_borders(content, 3).unwrap_err(), GroupBordersError::WrongFlag);
    }
}
