//! Port of `old_src/src/calibre/ebooks/rtf2xml/inline.py` (`Inline`).
//!
//! Wraps runs of character-formatted text (bold, italics, font-color,
//! ...) in `<inline>` tags. A bracket group's character-formatting
//! control words (`cw<ci<...`) are collected into a pending
//! [`InlineGroup`] as soon as its opening bracket is seen, but the
//! `<inline>` open tag itself is only written once actual text
//! arrives inside it (or, for groups that turn out to contain no
//! text before closing, never at all) -- "waiting" groups accumulate
//! (`groups_in_waiting`) across nested brackets and all flush together,
//! in open-order, the moment text is found. Closing mirrors this:
//! either an explicit `cb<nu<clos-brack` closes just its own group
//! (LIFO), or, if the paragraph ends first with the group still open,
//! [`InlineBuilder::end_para`] closes everything still pending.
//!
//! List text (`mi<mk<lst-tx-beg`/`lst-tx-end`) and body text keep
//! *entirely separate* group stacks and waiting-counts (`list_*` vs
//! `body_*` fields below) -- switching between them mid-document (even
//! mid-bracket-group) preserves each side's own independent state
//! rather than resetting it, matching Python's alternating
//! `self.__inline_list`/`self.__groups_in_waiting` aliasing between
//! two backing lists.
//!
//! # A documented upstream quirk, not this port's own
//!
//! `__after_open_bracket_func` checks `line[0:5] == 'cw<ci'` to decide
//! whether a line is a character-formatting control word -- Python's
//! own comment flags this as conflating `cw<ci` and `cw<pf` token
//! categories somewhere upstream. Preserved as-is; not something to
//! "fix" here.
//!
//! # Confirmed-dead state, omitted
//!
//! `self.__brac_count` (Python's own comment: `# do I need this?`)
//! and `self.__caps_list` are both assigned once and never read again
//! -- confirmed by grep, the same copy-paste-leftover pattern as every
//! other file in this follow-up issue so far.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use indexmap::IndexMap;
use thiserror::Error;

/// Port of the `run_level > 3`-gated `raise self.__bug_handler(msg)`
/// in `__write_inline`, reached if `groups_in_waiting` is nonzero
/// while the active inline-group stack is completely empty -- a
/// desync this port hasn't found a reachable trigger for either, kept
/// as a defensive check matching the original.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InlineError {
    #[error("self.__inline_list is []\n")]
    EmptyInlineList,
}

pub type Result<T> = std::result::Result<T, InlineError>;

const SPECIAL_TEXT_LINES: [&str; 7] = [
    "tx<mc<__________<rdblquote",
    "tx<mc<__________<ldblquote",
    "tx<mc<__________<lquote",
    "tx<mc<__________<rquote",
    "tx<mc<__________<emdash",
    "tx<mc<__________<endash",
    "tx<mc<__________<bullet",
];

fn token_info(line: &str) -> &str {
    if SPECIAL_TEXT_LINES.contains(&line) {
        "text"
    } else if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

fn char_dict(key: &str) -> Option<&'static str> {
    Some(match key {
        "annotation" => "annotation",
        "blue______" => "blue",
        "bold______" => "bold",
        "caps______" => "caps",
        "char-style" => "character-style",
        "dbl-strike" => "double-strike-through",
        "emboss____" => "emboss",
        "engrave___" => "engrave",
        "font-color" => "font-color",
        "font-down_" => "subscript",
        "font-size_" => "font-size",
        "font-style" => "font-style",
        "font-up___" => "superscript",
        "footnot-mk" => "footnote-marker",
        "green_____" => "green",
        "hidden____" => "hidden",
        "italics___" => "italics",
        "outline___" => "outline",
        "red_______" => "red",
        "shadow____" => "shadow",
        "small-caps" => "small-caps",
        "strike-thr" => "strike-through",
        "subscript_" => "subscript",
        "superscrip" => "superscript",
        "underlined" => "underlined",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    AfterOpenBracket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    InList,
    NotInList,
}

#[derive(Debug, Default)]
struct InlineGroup {
    contains_inline: bool,
    /// Port of `the_dict`, minus the `contains_inline` marker key
    /// (kept as its own field above instead) -- insertion order
    /// matters, it's written straight into the output tag.
    attrs: IndexMap<String, String>,
}

fn write_inline_open_tag(out: &mut String, group: &InlineGroup) {
    if let Some(face) = group.attrs.get("font-style") {
        out.push_str(&format!("mi<mk<font______<{face}\n"));
    }
    if let Some(value) = group.attrs.get("caps") {
        out.push_str(&format!("mi<mk<caps______<{value}\n"));
    }
    out.push_str("mi<tg<open-att__<inline");
    for (k, v) in &group.attrs {
        out.push_str(&format!("<{k}>{v}"));
    }
    out.push('\n');
}

struct InlineBuilder {
    state: State,
    place: Place,
    in_para: bool,
    list_inline_list: Vec<InlineGroup>,
    body_inline_list: Vec<InlineGroup>,
    groups_in_waiting_list: i32,
    groups_in_waiting_body: i32,
    run_level: u32,
}

impl InlineBuilder {
    fn new(run_level: u32) -> Self {
        Self {
            state: State::default(),
            place: Place::NotInList,
            in_para: false,
            list_inline_list: Vec::new(),
            body_inline_list: Vec::new(),
            groups_in_waiting_list: 0,
            groups_in_waiting_body: 0,
            run_level,
        }
    }

    fn inline_list(&mut self) -> &mut Vec<InlineGroup> {
        match self.place {
            Place::InList => &mut self.list_inline_list,
            Place::NotInList => &mut self.body_inline_list,
        }
    }

    fn inline_list_ref(&self) -> &Vec<InlineGroup> {
        match self.place {
            Place::InList => &self.list_inline_list,
            Place::NotInList => &self.body_inline_list,
        }
    }

    fn groups_in_waiting(&self) -> i32 {
        match self.place {
            Place::InList => self.groups_in_waiting_list,
            Place::NotInList => self.groups_in_waiting_body,
        }
    }

    fn set_groups_in_waiting(&mut self, v: i32) {
        match self.place {
            Place::InList => self.groups_in_waiting_list = v,
            Place::NotInList => self.groups_in_waiting_body = v,
        }
    }

    fn decr_groups_in_waiting(&mut self) {
        let v = self.groups_in_waiting();
        if v != 0 {
            self.set_groups_in_waiting(v - 1);
        }
    }

    /// Port of `__set_list_func`.
    fn set_list(&mut self, tok: &str) {
        match self.place {
            Place::InList => {
                if tok == "mi<mk<lst-tx-end" {
                    self.place = Place::NotInList;
                }
            }
            Place::NotInList => {
                if tok == "mi<mk<lst-tx-beg" {
                    self.place = Place::InList;
                }
            }
        }
    }

    /// Port of `__found_open_bracket_func`.
    fn found_open_bracket(&mut self) {
        self.state = State::AfterOpenBracket;
        let n = self.groups_in_waiting() + 1;
        self.set_groups_in_waiting(n);
        self.inline_list().push(InlineGroup::default());
    }

    /// Port of `__handle_control_word`.
    fn handle_control_word(&mut self, line: &str) {
        let char_info = if line.len() >= 16 { &line[6..16] } else { "" };
        let char_value = if line.len() >= 20 { &line[20..] } else { "" };
        if let Some(name) = char_dict(char_info) {
            let group = self.inline_list().last_mut().expect("after_open_bracket implies a pushed group");
            group.contains_inline = true;
            group.attrs.insert(name.to_string(), char_value.to_string());
        }
    }

    /// Port of `__close_bracket_func`.
    fn close_bracket(&mut self, out: &mut String) {
        if self.inline_list_ref().is_empty() {
            return;
        }
        let last = self.inline_list_ref().last().expect("just checked non-empty");
        let contains_inline = last.contains_inline;
        let has_font_style = last.attrs.contains_key("font-style");
        let has_caps = last.attrs.contains_key("caps");
        let waiting_zero = self.groups_in_waiting() == 0;
        let should_close = match self.place {
            Place::InList => contains_inline && waiting_zero,
            Place::NotInList => contains_inline && self.in_para && waiting_zero,
        };
        if should_close {
            out.push_str("mi<tg<close_____<inline\n");
            if has_font_style {
                out.push_str("mi<mk<font-end__\n");
            }
            if has_caps {
                out.push_str("mi<mk<caps-end__\n");
            }
        }
        self.inline_list().pop();
        self.decr_groups_in_waiting();
    }

    /// Port of `__found_text_func`.
    fn found_text(&mut self, out: &mut String) -> Result<()> {
        if self.place == Place::InList {
            self.write_inline(out)
        } else if !self.in_para {
            self.in_para = true;
            self.start_para(out);
            Ok(())
        } else if self.groups_in_waiting() != 0 {
            self.write_inline(out)
        } else {
            Ok(())
        }
    }

    /// Port of `__write_inline`.
    fn write_inline(&mut self, out: &mut String) -> Result<()> {
        let n = self.groups_in_waiting();
        if n != 0 {
            if self.inline_list_ref().is_empty() {
                if self.run_level > 3 {
                    return Err(InlineError::EmptyInlineList);
                }
                out.push_str("error\n");
                self.set_groups_in_waiting(0);
                return Ok(());
            }
            let len = self.inline_list_ref().len();
            let start = len.saturating_sub(n as usize);
            for idx in start..len {
                let group = &self.inline_list_ref()[idx];
                if group.contains_inline {
                    write_inline_open_tag(out, group);
                }
            }
        }
        self.set_groups_in_waiting(0);
        Ok(())
    }

    /// Port of `__end_para_func`.
    fn end_para(&mut self, out: &mut String) {
        if !self.in_para {
            return;
        }
        let n = self.groups_in_waiting();
        let len = self.inline_list_ref().len();
        let end = if n == 0 { len } else { len.saturating_sub(n as usize) };
        for idx in 0..end {
            let group = &self.inline_list_ref()[idx];
            if group.contains_inline {
                if group.attrs.contains_key("font-style") {
                    out.push_str("mi<mk<font-end__\n");
                }
                if group.attrs.contains_key("caps") {
                    out.push_str("mi<mk<caps-end__\n");
                }
                out.push_str("mi<tg<close_____<inline\n");
            }
        }
        self.in_para = false;
    }

    /// Port of `__start_para_func`.
    fn start_para(&mut self, out: &mut String) {
        let len = self.inline_list_ref().len();
        for idx in 0..len {
            let group = &self.inline_list_ref()[idx];
            if group.contains_inline {
                write_inline_open_tag(out, group);
            }
        }
        self.set_groups_in_waiting(0);
    }

    /// Port of `__default_func`.
    fn default_state(&mut self, out: &mut String, line: &str, tok: &str) -> Result<()> {
        match tok {
            "ob<nu<open-brack" => self.found_open_bracket(),
            "tx<nu<__________" | "tx<hx<__________" | "tx<ut<__________" | "mi<mk<inline-fld" | "text" => {
                self.found_text(out)?;
            }
            "cb<nu<clos-brack" => self.close_bracket(out),
            "mi<mk<par-end___" | "mi<mk<footnt-ope" | "mi<mk<footnt-ind" => self.end_para(out),
            _ => {}
        }
        out.push_str(line);
        out.push('\n');
        Ok(())
    }

    /// Port of `__after_open_bracket_func`.
    fn after_open_bracket(&mut self, out: &mut String, line: &str, tok: &str) -> Result<()> {
        if line.len() >= 5 && &line[..5] == "cw<ci" {
            self.handle_control_word(line);
        } else {
            match tok {
                "cb<nu<clos-brack" => {
                    self.state = State::Default;
                    self.close_bracket(out);
                }
                "tx<nu<__________" | "tx<hx<__________" | "tx<ut<__________" | "text" | "mi<mk<inline-fld" => {
                    self.state = State::Default;
                    self.found_text(out)?;
                }
                "ob<nu<open-brack" => {
                    self.state = State::Default;
                    self.found_open_bracket();
                }
                "mi<mk<par-end___" | "mi<mk<footnt-ope" | "mi<mk<footnt-ind" => {
                    self.state = State::Default;
                    self.end_para(out);
                }
                "cw<fd<field_____" => {
                    // Port of `__found_field_func`: a deliberate no-op
                    // whose only job is to keep this token from
                    // falling through unmatched -- see the Python
                    // docstring ("make sure I don't prematurely exit
                    // default state").
                    self.state = State::Default;
                }
                _ => {}
            }
        }
        out.push_str(line);
        out.push('\n');
        Ok(())
    }
}

/// Port of `Inline.form_tags`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn form_tags(content: &str, run_level: u32) -> Result<String> {
    let mut b = InlineBuilder::new(run_level);
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        b.set_list(tok);
        match b.state {
            State::Default => b.default_state(&mut out, line, tok)?,
            State::AfterOpenBracket => b.after_open_bracket(&mut out, line, tok)?,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_with_no_formatting_gets_no_inline_tags() {
        let content = "tx<nu<__________<hello\nmi<mk<par-end___\n";
        assert_eq!(form_tags(content, 1).unwrap(), content);
    }

    #[test]
    fn a_single_bold_run_closed_by_its_own_bracket() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ci<bold______<nu<true\n\
tx<nu<__________<hello\n\
cb<nu<clos-brack<0001\n\
mi<mk<par-end___\n";
        let out = form_tags(content, 1).unwrap();
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\n\
             cw<ci<bold______<nu<true\n\
             mi<tg<open-att__<inline<bold>true\n\
             tx<nu<__________<hello\n\
             mi<tg<close_____<inline\n\
             cb<nu<clos-brack<0001\n\
             mi<mk<par-end___\n"
        );
    }

    #[test]
    fn list_text_closes_via_close_bracket_not_end_of_paragraph() {
        let content = "\
mi<mk<lst-tx-beg\n\
ob<nu<open-brack<0001\n\
cw<ci<italics___<nu<true\n\
tx<nu<__________<hi\n\
cb<nu<clos-brack<0001\n\
mi<mk<lst-tx-end\n";
        let out = form_tags(content, 1).unwrap();
        assert_eq!(
            out,
            "mi<mk<lst-tx-beg\n\
             ob<nu<open-brack<0001\n\
             cw<ci<italics___<nu<true\n\
             mi<tg<open-att__<inline<italics>true\n\
             tx<nu<__________<hi\n\
             mi<tg<close_____<inline\n\
             cb<nu<clos-brack<0001\n\
             mi<mk<lst-tx-end\n"
        );
    }

    #[test]
    fn font_style_and_caps_get_their_own_marker_lines() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ci<font-style<nu<Arial\n\
cw<ci<caps______<nu<true\n\
tx<nu<__________<hi\n\
cb<nu<clos-brack<0001\n\
mi<mk<par-end___\n";
        let out = form_tags(content, 1).unwrap();
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\n\
             cw<ci<font-style<nu<Arial\n\
             cw<ci<caps______<nu<true\n\
             mi<mk<font______<Arial\n\
             mi<mk<caps______<true\n\
             mi<tg<open-att__<inline<font-style>Arial<caps>true\n\
             tx<nu<__________<hi\n\
             mi<tg<close_____<inline\n\
             mi<mk<font-end__\n\
             mi<mk<caps-end__\n\
             cb<nu<clos-brack<0001\n\
             mi<mk<par-end___\n"
        );
    }

    #[test]
    fn an_unclosed_group_is_closed_at_the_end_of_the_paragraph() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ci<bold______<nu<true\n\
tx<nu<__________<hi\n\
mi<mk<par-end___\n";
        let out = form_tags(content, 1).unwrap();
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\n\
             cw<ci<bold______<nu<true\n\
             mi<tg<open-att__<inline<bold>true\n\
             tx<nu<__________<hi\n\
             mi<tg<close_____<inline\n\
             mi<mk<par-end___\n"
        );
    }

    #[test]
    fn nested_waiting_groups_flush_together_in_open_order_when_text_arrives() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ci<bold______<nu<true\n\
ob<nu<open-brack<0002\n\
cw<ci<italics___<nu<true\n\
tx<nu<__________<hi\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n\
mi<mk<par-end___\n";
        let out = form_tags(content, 1).unwrap();
        assert_eq!(
            out,
            "ob<nu<open-brack<0001\n\
             cw<ci<bold______<nu<true\n\
             ob<nu<open-brack<0002\n\
             cw<ci<italics___<nu<true\n\
             mi<tg<open-att__<inline<bold>true\n\
             mi<tg<open-att__<inline<italics>true\n\
             tx<nu<__________<hi\n\
             mi<tg<close_____<inline\n\
             cb<nu<clos-brack<0002\n\
             mi<tg<close_____<inline\n\
             cb<nu<clos-brack<0001\n\
             mi<mk<par-end___\n"
        );
    }
}
