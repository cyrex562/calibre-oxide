//! Port of `old_src/src/calibre/ebooks/rtf2xml/group_styles.py`
//! (`GroupStyles`).
//!
//! Every `paragraph-definition` block arrives here still missing its
//! own `close` tag (an earlier pass leaves it open on purpose for
//! this one to add) -- so, either way, this pass is what inserts it.
//! What differs is what happens between two *consecutive*
//! same-`style-name` paragraph-definitions:
//! - `wrap = false` (the default): they're merged into one combined
//!   block -- only the first's own open tag and the last's own close
//!   tag survive; every later one's open tag in the run is dropped.
//! - `wrap = true`: each paragraph-definition keeps its own open and
//!   close tags, but the whole same-styled run is wrapped in one
//!   shared `<style-group name="...">` tag instead.
//!
//! # Confirmed-dead state, omitted
//!
//! `self.__left_indent`, `self.__list_type`, `self.__all_lists`,
//! `self.__name_regex`, `self.__found_appt`, and `self.__line_num`
//! are all assigned once in `__initiate_values` and never read again
//! anywhere -- confirmed by grep, the same copy-paste-leftover pattern
//! as [`super::make_lists`] and [`super::headings_to_sections`].
//! `__close_pard_` is defined but never called anywhere either. None
//! have a Rust counterpart.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use thiserror::Error;

/// Port of `__after_pard_func`'s `run_level > 2`-gated
/// `raise self.__bug_handler(msg)`, reached when a second
/// `paragraph-definition` close tag is seen with no intervening open
/// (the diagnostic `sys.stderr.write` fires unconditionally before
/// this gate, matching Python).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GroupStylesError {
    #[error("wrong flag")]
    WrongFlag,
}

pub type Result<T> = std::result::Result<T, GroupStylesError>;

const END_LIST: [&str; 13] = [
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    InPard,
    AfterPard,
}

struct GroupStylesBuilder {
    state: State,
    list_chunk: String,
    style_name: String,
    last_style_name: String,
    wrap: bool,
}

impl GroupStylesBuilder {
    fn get_style_name(&mut self, line: &str, tok: &str) {
        if tok == "mi<mk<style-name" {
            self.style_name = payload(line).to_string();
        }
    }

    fn write_start_wrap(&self, out: &mut String, name: &str) {
        if self.wrap {
            out.push_str(&format!(
                "mi<mk<style-grp_<{name}\nmi<tg<open-att__<style-group<name>{name}\nmi<mk<style_grp_<{name}\n"
            ));
        }
    }

    fn write_end_wrap(&self, out: &mut String) {
        if self.wrap {
            out.push_str("mi<mk<style_gend\nmi<tg<close_____<style-group\nmi<mk<stylegend_\n");
        }
    }

    /// Port of `__default_func`.
    fn default(&mut self, out: &mut String, line: &str, tok: &str) {
        if is_pard_open(line, tok) {
            self.state = State::InPard;
            self.last_style_name = self.style_name.clone();
            let last_style_name = self.last_style_name.clone();
            self.write_start_wrap(out, &last_style_name);
        }
        out.push_str(line);
        out.push('\n');
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
        if self.last_style_name == self.style_name {
            if self.wrap {
                out.push_str("mi<tg<close_____<paragraph-definition\n");
            }
            out.push_str(&self.list_chunk);
            self.list_chunk.clear();
            self.state = State::InPard;
            if self.wrap {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            out.push_str("mi<tg<close_____<paragraph-definition\n");
            self.write_end_wrap(out);
            out.push_str(&self.list_chunk);
            let style_name = self.style_name.clone();
            self.write_start_wrap(out, &style_name);
            out.push_str(line);
            out.push('\n');
            self.state = State::InPard;
            self.last_style_name = self.style_name.clone();
            self.list_chunk.clear();
        }
    }

    /// Port of `__after_pard_func`.
    fn after_pard(&mut self, out: &mut String, line: &str, tok: &str, run_level: u32) -> Result<()> {
        if is_pard_open(line, tok) {
            self.pard_after_par_def(out, line);
        } else if is_pard_close(line, tok) {
            eprintln!("Wrong flag in __after_pard_func");
            if run_level > 2 {
                return Err(GroupStylesError::WrongFlag);
            }
        } else if END_LIST.contains(&tok) {
            out.push_str("mi<tg<close_____<paragraph-definition\n");
            self.write_end_wrap(out);
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

/// Port of `GroupStyles.group_styles`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn group_styles(content: &str, wrap: bool, run_level: u32) -> Result<String> {
    let mut b = GroupStylesBuilder {
        state: State::default(),
        list_chunk: String::new(),
        style_name: String::new(),
        last_style_name: String::new(),
        wrap,
    };
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        b.get_style_name(line, tok);

        match b.state {
            State::Default => b.default(&mut out, line, tok),
            State::InPard => b.in_pard(&mut out, line, tok),
            State::AfterPard => b.after_pard(&mut out, line, tok, run_level)?,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pard(style: &str, align: &str, text: &str) -> String {
        format!(
            "mi<mk<style-name<{style}\nmi<tg<open-att__<paragraph-definition<align>{align}\ntx<nu<__________<{text}\nmi<tg<close_____<paragraph-definition\n"
        )
    }

    #[test]
    fn plain_content_with_no_paragraph_definitions_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\nmi<mk<tabl-start\n";
        assert_eq!(group_styles(content, false, 1).unwrap(), content);
    }

    #[test]
    fn unwrapped_consecutive_same_style_paragraphs_are_merged_into_one_block() {
        let mut content = pard("Normal", "left", "para one");
        content.push_str(&pard("Normal", "left", "para two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_styles(&content, false, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<style-name<Normal\n\
             mi<tg<open-att__<paragraph-definition<align>left\n\
             tx<nu<__________<para one\n\
             mi<mk<style-name<Normal\n\
             tx<nu<__________<para two\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<tabl-start\n"
        );
    }

    #[test]
    fn unwrapped_consecutive_different_style_paragraphs_stay_separate() {
        let mut content = pard("Normal", "left", "para one");
        content.push_str(&pard("Heading", "center", "para two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_styles(&content, false, 1).unwrap();
        // Every input line is preserved verbatim -- no merge happens.
        assert_eq!(out, content);
    }

    #[test]
    fn wrapped_consecutive_same_style_paragraphs_share_one_style_group_but_stay_separate() {
        let mut content = pard("Normal", "left", "para one");
        content.push_str(&pard("Normal", "left", "para two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_styles(&content, true, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<style-name<Normal\n\
             mi<mk<style-grp_<Normal\n\
             mi<tg<open-att__<style-group<name>Normal\n\
             mi<mk<style_grp_<Normal\n\
             mi<tg<open-att__<paragraph-definition<align>left\n\
             tx<nu<__________<para one\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<style-name<Normal\n\
             mi<tg<open-att__<paragraph-definition<align>left\n\
             tx<nu<__________<para two\n\
             mi<tg<close_____<paragraph-definition\n\
             mi<mk<style_gend\n\
             mi<tg<close_____<style-group\n\
             mi<mk<stylegend_\n\
             mi<mk<tabl-start\n"
        );
    }

    #[test]
    fn wrapped_consecutive_different_style_paragraphs_get_their_own_style_groups() {
        let mut content = pard("Normal", "left", "para one");
        content.push_str(&pard("Heading", "center", "para two"));
        content.push_str("mi<mk<tabl-start\n");
        let out = group_styles(&content, true, 1).unwrap();
        assert_eq!(out.matches("mi<tg<open-att__<style-group").count(), 2, "{out}");
        assert_eq!(out.matches("mi<tg<close_____<style-group\n").count(), 2, "{out}");
    }

    #[test]
    fn a_double_close_tag_is_a_wrong_flag_error_only_at_high_run_level() {
        let content = "\
mi<tg<open-att__<paragraph-definition<align>left\n\
tx<nu<__________<hi\n\
mi<tg<close_____<paragraph-definition\n\
mi<tg<close_____<paragraph-definition\n";
        assert!(group_styles(content, false, 2).is_ok());
        assert_eq!(group_styles(content, false, 3).unwrap_err(), GroupStylesError::WrongFlag);
    }
}
