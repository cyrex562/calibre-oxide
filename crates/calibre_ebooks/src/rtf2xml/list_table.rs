//! Port of `old_src/src/calibre/ebooks/rtf2xml/list_table.py`
//! (`ListTable`).
//!
//! Parses a `\listtable` block's tokens into `<list-in-table>`/
//! `<level-in-table>` tags. Unlike this issue's other passes,
//! [`parse_list_table`] doesn't operate line-by-line over the whole
//! document -- it's handed just the `\listtable` block's own text (one
//! call per block) and returns the fully-built replacement string plus
//! a [`ListInfo`] per list found, mirroring Python's `(list_table_final,
//! all_lists)` tuple return.
//!
//! # Confirmed-dead state, omitted
//!
//! Three pieces of Python state are write-only -- assigned but never
//! read back, confirmed by grep: `self.__ob_group` (incremented/
//! decremented, never compared against anything), `self.__level_text_string`
//! (reset to `''` at group boundaries, never appended to), and
//! `bullet_text` inside `__write_final_string` (accumulated from
//! suppressed bullet attributes, but its only use -- appending
//! `<bullet-type>...` -- is commented out). None of the three are
//! ported; this module still reproduces every *observable* effect
//! (the bullet-suppression skip itself is real and preserved -- see
//! [`ListTableBuilder::write_final_string`]).
//!
//! # A discarded diagnostic, restored
//!
//! `__after_bracket_func`'s `run_level > 3` branch builds a `msg`
//! string and then does `raise self.__bug_handler` -- raising the
//! handler *class* with no arguments, rather than
//! `raise self.__bug_handler(msg)`, so the constructed message is
//! silently discarded in the original. [`ListTableError::NoMatchingToken`]
//! includes it anyway: unlike [`super::table`]'s list-vs-int bug (fixed
//! because literal preservation would make the whole pass fail on an
//! ordinary case), this is a lower-stakes, purely cosmetic
//! discrepancy in an already-erroring path -- there's no reason to
//! deliberately make the port's error message less useful than an
//! obvious, no-behavioral-cost read of the intent.
//!
//! # Everything else
//!
//! `self.__final_dict` and `self.__list_dict` (assigned in
//! `__initiate_values`, never referenced again anywhere in the class)
//! are dead too and have no Rust counterpart at all.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use indexmap::IndexMap;
use thiserror::Error;

/// Port of the `run_level > 3`-gated `raise self.__bug_handler` in
/// `__after_bracket_func` -- see this module's own doc for why the
/// discarded diagnostic message is restored here.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ListTableError {
    #[error("No matching token after open bracket\ntoken is \"{0}\n\"")]
    NoMatchingToken(String),
}

pub type Result<T> = std::result::Result<T, ListTableError>;

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 { line[line.len() - 4..].to_string() } else { line.to_string() }
}

fn value_field(line: &str) -> &str {
    if line.len() >= 20 { &line[20..] } else { "" }
}

fn main_list_dict(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "cw<ls<ls-tem-id_" => "list-template-id",
        "cw<ls<list-hybri" => "list-hybrid",
        "cw<ls<lis-tbl-id" => "list-table-id",
        _ => return None,
    })
}

fn level_dict(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "cw<ls<level-star" => "list-number-start",
        "cw<ls<level-spac" => "list-space",
        "cw<ls<level-inde" => "level-indent",
        "cw<ls<fir-ln-ind" => "first-line-indent",
        "cw<ls<left-inden" => "left-indent",
        "cw<ls<tab-stop__" => "tabs",
        "cw<ls<level-type" => "numbering-type",
        "cw<pf<right-inde" => "right-indent",
        "cw<pf<left-inden" => "left-indent",
        "cw<pf<fir-ln-ind" => "first-line-indent",
        "cw<ci<italics___" => "italics",
        "cw<ci<bold______" => "bold",
        "cw<ss<para-style" => "paragraph-style-name",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    Level,
    List,
    UnsureOb,
    LevelNumber,
    LevelText,
    ListName,
}

/// One `\list` entry: its own attributes (`list-template-id`,
/// `list-hybrid`, `list-table-id`) plus one attribute dict per
/// `\listlevel` found inside it, in document order. Python's
/// `'list-id': []` placeholder entry (never populated, always
/// excluded from output) has no counterpart here at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListInfo {
    pub attributes: IndexMap<String, String>,
    pub levels: Vec<IndexMap<String, String>>,
}

#[derive(Default)]
struct ListTableBuilder {
    state: State,
    all_lists: Vec<ListInfo>,
    level_text_position: Option<String>,
    prefix_string: Option<String>,
    level_numbers_string: String,
    found_level_text_length: bool,
    list_ob_count: String,
    level_ob_count: String,
    level_number_ob_count: String,
    level_text_ob_count: String,
    list_name_ob_count: String,
}

impl ListTableBuilder {
    fn current_list_mut(&mut self) -> &mut ListInfo {
        self.all_lists.last_mut().expect("List/Level states imply a pushed ListInfo")
    }

    fn current_level_mut(&mut self) -> &mut IndexMap<String, String> {
        self.current_list_mut().levels.last_mut().expect("Level states imply a pushed level")
    }

    fn found_list(&mut self, ob_count: &str) {
        self.state = State::List;
        self.list_ob_count = ob_count.to_string();
        self.all_lists.push(ListInfo::default());
    }

    fn found_level(&mut self, ob_count: &str) {
        self.state = State::Level;
        self.level_ob_count = ob_count.to_string();
        self.current_list_mut().levels.push(IndexMap::new());
    }

    /// Port of `__after_bracket_func`.
    fn after_bracket(&mut self, line: &str, tok: &str, ob_count: &str, run_level: u32) -> Result<()> {
        match tok {
            "cw<ls<level-text" => {
                self.state = State::LevelText;
                self.level_text_ob_count = ob_count.to_string();
            }
            "cw<ls<level-numb" => {
                self.level_number_ob_count = ob_count.to_string();
                self.state = State::LevelNumber;
            }
            "cw<ls<list-tb-le" => self.found_level(ob_count),
            "cw<ls<list-in-tb" => self.found_list(ob_count),
            "cw<ls<list-name_" => {
                self.state = State::ListName;
                self.list_name_ob_count = ob_count.to_string();
            }
            _ => {
                if run_level > 3 {
                    return Err(ListTableError::NoMatchingToken(line.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Port of `__list_func`.
    fn in_list(&mut self, line: &str, tok: &str, cb_count: &str) {
        if tok == "cb<nu<clos-brack" && cb_count == self.list_ob_count {
            self.state = State::Default;
        } else if tok == "ob<nu<open-brack" {
            self.state = State::UnsureOb;
        } else if let Some(att) = main_list_dict(tok) {
            let value = value_field(line).to_string();
            self.current_list_mut().attributes.insert(att.to_string(), value);
        }
    }

    /// Port of `__level_func`.
    fn in_level(&mut self, line: &str, tok: &str, cb_count: &str) {
        if tok == "cb<nu<clos-brack" && cb_count == self.level_ob_count {
            self.state = State::List;
        } else if tok == "ob<nu<open-brack" {
            self.state = State::UnsureOb;
        } else if let Some(att) = level_dict(tok) {
            let value = value_field(line).to_string();
            self.current_level_mut().insert(att.to_string(), value);
        }
    }

    /// Port of `__level_number_func`.
    fn in_level_number(&mut self, line: &str, tok: &str, cb_count: &str) {
        if tok == "cb<nu<clos-brack" && cb_count == self.level_number_ob_count {
            self.state = State::Level;
            let numbers = std::mem::take(&mut self.level_numbers_string);
            self.current_level_mut().insert("level-numbers".to_string(), numbers);
        } else if tok == "tx<hx<__________" {
            let payload = if line.len() >= 18 { &line[18..] } else { "" };
            self.level_numbers_string.push_str(&format!("\\&#x0027;{payload}"));
        } else if tok == "tx<nu<__________" {
            let payload = if line.len() >= 17 { &line[17..] } else { "" };
            self.level_numbers_string.push_str(payload);
        }
    }

    /// Port of `__level_text_func`.
    fn in_level_text(&mut self, line: &str, tok: &str, cb_count: &str) {
        if tok == "cb<nu<clos-brack" && cb_count == self.level_text_ob_count {
            if let Some(prefix) = self.prefix_string.clone() {
                let is_bullet =
                    self.current_level_mut().get("numbering-type").map(String::as_str) == Some("bullet");
                if is_bullet {
                    let cleaned = prefix.replace('_', "");
                    self.current_level_mut().insert("bullet-type".to_string(), cleaned);
                }
            }
            self.state = State::Level;
            self.found_level_text_length = false;
        } else if tok == "tx<hx<__________" {
            self.parse_level_text_length(line);
        } else if tok == "tx<nu<__________" {
            let mut text = if line.len() >= 17 { line[17..].to_string() } else { String::new() };
            if text.ends_with(';') {
                text = text.replace(';', "");
            }
            match self.level_text_position.clone() {
                None => self.prefix_string = Some(text),
                Some(pos) => {
                    self.current_level_mut().insert(pos, text);
                }
            }
        } else if tok == "cw<ls<lv-tem-id_" {
            let value = value_field(line).to_string();
            self.current_level_mut().insert("level-template-id".to_string(), value);
        }
    }

    /// Port of `__parse_level_text_length`.
    fn parse_level_text_length(&mut self, line: &str) {
        let num_str = if line.len() >= 18 { &line[18..] } else { "" };
        let Ok(num) = i64::from_str_radix(num_str.trim(), 16) else { return };
        if !self.found_level_text_length {
            self.current_level_mut().insert("list-text-length".to_string(), num.to_string());
            self.found_level_text_length = true;
        } else {
            let num = num + 1;
            let level_marker = format!("level{num}-suffix");
            let show_marker = format!("show-level{num}");
            self.level_text_position = Some(level_marker);
            self.current_level_mut().insert(show_marker, "true".to_string());
            if let Some(prefix) = self.prefix_string.take() {
                let prefix_marker = format!("level{num}-prefix");
                self.current_level_mut().insert(prefix_marker, prefix);
            }
        }
    }

    /// Port of `__list_name_func`: the list name's own text (if any)
    /// is never captured anywhere -- only the group's own closing
    /// bracket is checked for.
    fn in_list_name(&mut self, tok: &str, cb_count: &str) {
        if tok == "cb<nu<clos-brack" && cb_count == self.list_name_ob_count {
            self.state = State::List;
        }
    }

    /// Port of `__write_final_string`.
    fn write_final_string(&self) -> String {
        let mut out = String::from("mi<mk<listabbeg_\n");
        // Preserved upstream quirk: the real `+=` re-reads its own
        // old value, so the `mi<mk<listabbeg_` marker ends up
        // duplicated in the output (see this module's own doc).
        let prefix = out.clone();
        out.push_str(&format!("mi<tg<open______<list-table\nmi<mk<listab-beg\n{prefix}"));

        for list in &self.all_lists {
            out.push_str("mi<tg<open-att__<list-in-table");
            for (k, v) in &list.attributes {
                out.push_str(&format!("<{k}>{v}"));
            }
            out.push('\n');

            for (idx, level) in list.levels.iter().enumerate() {
                let level_num = idx + 1;
                out.push_str("mi<tg<empty-att_<level-in-table");
                out.push_str(&format!("<level>{level_num}"));
                let is_bullet = level.get("numbering-type").map(String::as_str) == Some("bullet");
                for (k, v) in level {
                    if is_bullet && (k.starts_with("show-level") || k.ends_with("suffix") || k.ends_with("prefix"))
                    {
                        continue;
                    }
                    out.push_str(&format!("<{k}>{v}"));
                }
                out.push('\n');
            }
            out.push_str("mi<tg<close_____<list-in-table\n");
        }
        out.push_str("mi<mk<listab-end\nmi<tg<close_____<list-table\nmi<mk<listabend_\n");
        out
    }
}

/// Port of `ListTable.parse_list_table`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn parse_list_table(content: &str, run_level: u32) -> Result<(String, Vec<ListInfo>)> {
    let mut b = ListTableBuilder::default();
    let mut ob_count = String::new();
    let mut cb_count = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        if tok == "ob<nu<open-brack" {
            ob_count = last_four(line);
        }
        if tok == "cb<nu<clos-brack" {
            cb_count = last_four(line);
        }

        match b.state {
            State::Default => {
                if tok == "ob<nu<open-brack" {
                    b.state = State::UnsureOb;
                }
            }
            State::UnsureOb => b.after_bracket(line, tok, &ob_count, run_level)?,
            State::List => b.in_list(line, tok, &cb_count),
            State::Level => b.in_level(line, tok, &cb_count),
            State::LevelNumber => b.in_level_number(line, tok, &cb_count),
            State::LevelText => b.in_level_text(line, tok, &cb_count),
            State::ListName => b.in_list_name(tok, &cb_count),
        }
    }

    let final_string = b.write_final_string();
    Ok((final_string, b.all_lists))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_with_one_level_produces_list_and_level_tags() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<list-in-tb<nu<true\n\
cw<ls<ls-tem-id_<nu<12345\n\
ob<nu<open-brack<0002\n\
cw<ls<list-tb-le<nu<true\n\
cw<ls<level-type<nu<arabic\n\
cw<ls<level-inde<nu<240\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n";
        let (out, lists) = parse_list_table(content, 1).unwrap();
        assert!(out.contains("mi<tg<open-att__<list-in-table<list-template-id>12345\n"), "{out}");
        assert!(
            out.contains("mi<tg<empty-att_<level-in-table<level>1<numbering-type>arabic<level-indent>240\n"),
            "{out}"
        );
        assert!(out.contains("mi<tg<close_____<list-in-table\n"), "{out}");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].attributes.get("list-template-id").map(String::as_str), Some("12345"));
    }

    #[test]
    fn level_numbers_accumulate_hex_and_plain_text_tokens() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<list-in-tb<nu<true\n\
ob<nu<open-brack<0002\n\
cw<ls<list-tb-le<nu<true\n\
ob<nu<open-brack<0003\n\
cw<ls<level-numb<nu<true\n\
tx<hx<__________<'01\n\
tx<nu<__________<A\n\
cb<nu<clos-brack<0003\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n";
        let (out, _) = parse_list_table(content, 1).unwrap();
        assert!(out.contains("<level-numbers>\\&#x0027;01A"), "{out}");
    }

    #[test]
    fn list_name_text_is_ignored_only_its_closing_bracket_matters() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<list-in-tb<nu<true\n\
ob<nu<open-brack<0002\n\
cw<ls<list-name_<nu<true\n\
tx<nu<__________<MyListName\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n";
        let (out, lists) = parse_list_table(content, 1).unwrap();
        assert!(!out.contains("MyListName"), "{out}");
        assert!(lists[0].attributes.is_empty());
    }

    #[test]
    fn a_bullet_level_derives_bullet_type_from_the_prefix_at_close() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<list-in-tb<nu<true\n\
ob<nu<open-brack<0002\n\
cw<ls<list-tb-le<nu<true\n\
cw<ls<level-type<nu<bullet\n\
ob<nu<open-brack<0003\n\
cw<ls<level-text<nu<true\n\
tx<hx<__________<'02\n\
tx<nu<__________<_-;\n\
cb<nu<clos-brack<0003\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n";
        let (out, _) = parse_list_table(content, 1).unwrap();
        assert!(out.contains("<bullet-type>-"), "{out}");
    }

    #[test]
    fn a_bullet_level_suppresses_show_level_and_suffix_prefix_attributes() {
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<list-in-tb<nu<true\n\
ob<nu<open-brack<0002\n\
cw<ls<list-tb-le<nu<true\n\
cw<ls<level-type<nu<bullet\n\
ob<nu<open-brack<0003\n\
cw<ls<level-text<nu<true\n\
tx<hx<__________<'02\n\
tx<nu<__________<pre-;\n\
tx<hx<__________<'01\n\
cb<nu<clos-brack<0003\n\
cb<nu<clos-brack<0002\n\
cb<nu<clos-brack<0001\n";
        let (out, _) = parse_list_table(content, 1).unwrap();
        assert!(out.contains("<numbering-type>bullet"), "{out}");
        assert!(out.contains("<list-text-length>2"), "{out}");
        assert!(!out.contains("show-level"), "{out}");
        assert!(!out.contains("level2-prefix"), "{out}");
    }

    #[test]
    fn an_unrecognized_token_after_a_bracket_is_ignored_at_low_run_level_and_errors_at_high() {
        let content = "ob<nu<open-brack<0001\ncw<ls<unknown-tok<nu<true\n";
        assert!(parse_list_table(content, 1).is_ok());
        assert!(matches!(
            parse_list_table(content, 4),
            Err(ListTableError::NoMatchingToken(_))
        ));
    }
}
