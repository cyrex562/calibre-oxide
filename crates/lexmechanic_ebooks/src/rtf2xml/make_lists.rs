//! Port of `old_src/src/calibre/ebooks/rtf2xml/make_lists.py`
//! (`MakeLists`).
//!
//! Wraps consecutive `paragraph-definition` tags that carry an
//! embedded `<list-id>NNN>` attribute (written by an earlier,
//! out-of-scope pass) in `<list>`/`<item>` tags, using each
//! paragraph's left-indent to decide whether it starts a new nested
//! list, continues the current list's item, or closes back out to a
//! shallower list.
//!
//! Consumes [`super::list_table::ListInfo`]/[`super::override_table`]'s
//! combined output (issue #188's earlier follow-up) to determine each
//! list's `list-type` (`ordered`/`unordered`, derived from a level's
//! `numbering-type` attribute).
//!
//! # A crash averted by this port's different data shape
//!
//! `__write_start_list` computes `level = int(self.__level) + 1` and
//! clamps it to `len(curlist) - 1` when too large, where Python's
//! `curlist` is `[list_attrs_dict, [level0_dict], [level1_dict], ...]`
//! -- so `len(curlist) - 1` is `0` whenever a list has *zero* defined
//! levels, making the clamped index select `curlist[0]` (the list's
//! own attribute dict, not a level!) and then crash trying to
//! subscript it with `[0]` as if it were a one-element wrapper list.
//! This port's [`ListInfo`] keeps `attributes` and `levels` as
//! separate fields rather than one Python-style mixed list, so there's
//! no equivalent mis-indexing possible; an empty `levels` here just
//! degrades to "no level data available" (falls back to `ordered`)
//! rather than crashing -- a difference in the port's representation,
//! not a deliberate behavioral fix of an RTF-semantics bug.
//!
//! # Confirmed-dead state, omitted
//!
//! `self.__found_appt` and `self.__line_num` gate a diagnostic in
//! `__close_lists` (`if self.__line_num < 25 and self.__found_appt:`)
//! but both are set once, to `0`, and never changed again -- confirmed
//! by grep. The diagnostic can never fire; neither field nor the
//! `eprintln!` it would have gated are ported.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use indexmap::IndexMap;
use lazy_static::lazy_static;
use regex::Regex;

use super::list_table::ListInfo;

lazy_static! {
    static ref ID_REGEX: Regex = Regex::new(r"<list-id>(\d+)").unwrap();
    static ref LV_REGEX: Regex = Regex::new(r"<list-level>(\d+)").unwrap();
}

const HEADINGS: [&str; 9] = [
    "heading 1",
    "heading 2",
    "heading 3",
    "heading 4",
    "heading 5",
    "heading 6",
    "heading 7",
    "heading 8",
    "heading 9",
];

const ALLOW_LEVELS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

const END_LIST: [&str; 15] = [
    "mi<mk<body-close",
    "mi<mk<par-in-fld",
    "cw<tb<cell______",
    "cw<tb<row-def___",
    "cw<tb<row_______",
    "mi<mk<sect-close",
    "mi<mk<sect-start",
    "mi<mk<header-beg",
    "mi<mk<header-end",
    "mi<mk<head___clo",
    "mi<mk<fldbk-end_",
    "mi<mk<close_cell",
    "mi<mk<footnt-ope",
    "mi<mk<foot___clo",
    "mi<mk<tabl-start",
];

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn payload(line: &str) -> &str {
    if line.len() >= 17 { &line[17..] } else { "" }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    InPard,
    AfterPard,
}

#[derive(Debug, Clone)]
struct OpenList {
    left_indent: f64,
    id: String,
}

/// Optional flags `MakeLists.__init__` takes, bundled to keep
/// [`make_lists`]'s own signature from growing an unwieldy run of
/// bare `bool`/`u32` parameters.
#[derive(Debug, Clone, Copy)]
pub struct MakeListsOptions {
    pub headings_to_sections: bool,
    pub no_headings_as_list: bool,
    pub write_list_info: bool,
    pub run_level: u32,
}

impl Default for MakeListsOptions {
    fn default() -> Self {
        Self { headings_to_sections: false, no_headings_as_list: true, write_list_info: false, run_level: 1 }
    }
}

struct MakeListsBuilder<'a> {
    state: State,
    left_indent: f64,
    list_type: String,
    all_lists: Vec<OpenList>,
    level: String,
    list_chunk: String,
    style_name: String,
    list_of_lists: &'a [ListInfo],
    opts: MakeListsOptions,
}

impl MakeListsBuilder<'_> {
    fn is_a_heading(&self) -> bool {
        if HEADINGS.contains(&self.style_name.as_str()) {
            self.opts.headings_to_sections || self.opts.no_headings_as_list
        } else {
            false
        }
    }

    /// Port of `__get_index_of_list`.
    fn get_index_of_list(&self, id: &str) -> Option<usize> {
        if id == "0" {
            return None;
        }
        let found = self.list_of_lists.iter().position(|l| l.list_id.iter().any(|x| x == id));
        if found.is_none() && self.opts.run_level > 0 {
            eprintln!(
                "Module is make_lists.py\nMethod is __get_index_of_list\nThe main list does not appear to have a matching id for {id} \n"
            );
        }
        found
    }

    fn get_indent(&mut self, line: &str, tok: &str) {
        if tok == "mi<mk<left_inden" {
            if let Ok(v) = payload(line).trim().parse::<f64>() {
                self.left_indent = v;
            }
        }
    }

    fn get_list_type(&mut self, line: &str, tok: &str) {
        if tok == "mi<mk<list-type_" {
            let p = payload(line);
            self.list_type = if p == "item" { "unordered".to_string() } else { p.to_string() };
        }
    }

    fn get_style_name(&mut self, line: &str, tok: &str) {
        if tok == "mi<mk<style-name" {
            self.style_name = payload(line).to_string();
        }
    }

    fn write_start_item(&self, out: &mut String) {
        out.push_str("mi<mk<item_start\nmi<tg<open______<item\nmi<mk<itemstart_\n");
    }

    fn write_end_item(&self, out: &mut String) {
        out.push_str("mi<tg<item_end__\nmi<tg<close_____<item\nmi<tg<item__end_\n");
    }

    fn write_end_list(&self, out: &mut String) {
        out.push_str("mi<tg<close_____<list\nmi<mk<list_close\n");
    }

    /// Port of `__write_start_list`.
    fn write_start_list(&mut self, out: &mut String, id: &str) {
        self.all_lists.push(OpenList { left_indent: self.left_indent, id: id.to_string() });
        out.push_str("mi<mk<list_start\n");
        let lev_num: &str = if ALLOW_LEVELS.contains(&self.level.as_str()) { &self.level } else { "0" };
        out.push_str(&format!("mi<tg<open-att__<list<list-id>{id}<level>{lev_num}"));

        let mut matched: Option<(&IndexMap<String, String>, Option<&IndexMap<String, String>>)> = None;
        if self.list_of_lists.is_empty() {
            out.push_str(&format!("<list-type>{}", self.list_type));
        } else if let Some(idx) = self.get_index_of_list(id) {
            let curlist = &self.list_of_lists[idx];
            let raw_idx: usize = self.level.parse().unwrap_or(0);
            let level_dict = if curlist.levels.is_empty() {
                None
            } else {
                let clamped = raw_idx.min(curlist.levels.len() - 1);
                curlist.levels.get(clamped)
            };
            let list_type = match level_dict.and_then(|d| d.get("numbering-type")).map(String::as_str) {
                Some("bullet") => "unordered",
                _ => "ordered",
            };
            out.push_str(&format!("<list-type>{list_type}"));
            matched = Some((&curlist.attributes, level_dict));
        } else {
            out.push_str(&format!("<list-type>{}", self.list_type));
        }

        if self.opts.write_list_info {
            if let Some((list_dict, level_dict)) = matched {
                if !list_dict.is_empty() {
                    for (k, v) in list_dict {
                        if k == "list-id" {
                            continue;
                        }
                        out.push_str(&format!("<{k}>{v}"));
                    }
                    if let Some(ld) = level_dict {
                        for (k, v) in ld {
                            out.push_str(&format!("<{k}>{v}"));
                        }
                    }
                }
            }
        }
        out.push('\n');
        out.push_str("mi<mk<liststart_\n");
        self.write_start_item(out);
    }

    /// Port of `__close_lists`. Iterates every currently-open list
    /// (most-recently-opened first), closing each whose own indent is
    /// at or beyond the current paragraph's, then drops exactly that
    /// many entries off the front -- preserved literally even though
    /// this assumes the closed entries always form a contiguous
    /// prefix once reversed (true for normally-nested lists, but not
    /// enforced).
    fn close_lists(&mut self, out: &mut String) {
        let current_indent = self.left_indent;
        self.all_lists.reverse();
        let mut num_levels_closed = 0;
        for entry in &self.all_lists {
            if current_indent <= entry.left_indent {
                self.write_end_item(out);
                self.write_end_list(out);
                num_levels_closed += 1;
            }
        }
        self.all_lists.drain(0..num_levels_closed);
        self.all_lists.reverse();
    }

    /// Port of `__list_after_par_def_func`.
    fn list_after_par_def(&mut self, out: &mut String, id: &str) {
        let last = self.all_lists.last().expect("in_pard/after_pard states imply a non-empty list stack");
        let last_id = last.id.clone();
        let last_indent = last.left_indent;
        if id != last_id {
            self.close_lists(out);
            out.push_str(&self.list_chunk);
            self.write_start_list(out, id);
        } else if self.left_indent > last_indent {
            out.push_str(&self.list_chunk);
            self.write_start_list(out, id);
        } else {
            self.write_end_item(out);
            out.push_str(&self.list_chunk);
            self.write_start_item(out);
        }
        self.list_chunk.clear();
    }

    /// Port of `__default_func`.
    fn default(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<tg<open-att__" && line.len() >= 37 && &line[17..37] == "paragraph-definition" {
            if !self.is_a_heading() {
                if let Some(cap) = ID_REGEX.captures(line) {
                    let num = cap[1].to_string();
                    self.state = State::InPard;
                    if let Some(lv) = LV_REGEX.captures(line) {
                        self.level = lv[1].to_string();
                    }
                    self.write_start_list(out, &num);
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    /// Port of `__in_pard_func`.
    fn in_pard(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<mk<pard-end__" {
            self.state = State::AfterPard;
        }
        out.push_str(line);
        out.push('\n');
    }

    /// Port of `__after_pard_func`.
    fn after_pard(&mut self, out: &mut String, line: &str, tok: &str) {
        if tok == "mi<tg<open-att__" && line.len() >= 37 && &line[17..37] == "paragraph-definition" {
            let is_heading = self.is_a_heading();
            if is_heading {
                self.left_indent = -1000.0;
                self.close_lists(out);
                out.push_str(&self.list_chunk);
                self.list_chunk.clear();
                self.state = State::Default;
                out.push_str(line);
                out.push('\n');
            } else if let Some(cap) = ID_REGEX.captures(line) {
                if let Some(lv) = LV_REGEX.captures(line) {
                    self.level = lv[1].to_string();
                }
                let num = cap[1].to_string();
                self.list_after_par_def(out, &num);
                out.push_str(line);
                out.push('\n');
                self.state = State::InPard;
            } else {
                self.close_lists(out);
                out.push_str(&self.list_chunk);
                self.list_chunk.clear();
                out.push_str(line);
                out.push('\n');
                self.state = if self.all_lists.is_empty() { State::Default } else { State::InPard };
            }
        } else if END_LIST.contains(&tok) {
            self.left_indent = -1000.0;
            self.close_lists(out);
            out.push_str(&self.list_chunk);
            self.list_chunk.clear();
            self.state = State::Default;
            out.push_str(line);
            out.push('\n');
        } else {
            self.list_chunk.push_str(line);
            self.list_chunk.push('\n');
        }
    }
}

/// Port of `MakeLists.make_lists`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file. `list_of_lists` should be
/// [`super::list_table::parse_list_table`] / [`super::override_table::parse_override_table`]'s
/// combined output, in the same order.
pub fn make_lists(content: &str, list_of_lists: &[ListInfo], opts: MakeListsOptions) -> String {
    let mut b = MakeListsBuilder {
        state: State::default(),
        left_indent: 0.0,
        list_type: "not-defined".to_string(),
        all_lists: Vec::new(),
        level: "0".to_string(),
        list_chunk: String::new(),
        style_name: String::new(),
        list_of_lists,
        opts,
    };
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        b.get_indent(line, tok);
        b.get_list_type(line, tok);
        b.get_style_name(line, tok);

        match b.state {
            State::Default => b.default(&mut out, line, tok),
            State::InPard => b.in_pard(&mut out, line, tok),
            State::AfterPard => b.after_pard(&mut out, line, tok),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pard(id: &str, level: &str, indent: f64) -> String {
        format!(
            "mi<mk<left_inden<{indent}\nmi<tg<open-att__<paragraph-definition<list-id>{id}<list-level>{level}\nmi<mk<pard-end__\n"
        )
    }

    #[test]
    fn a_plain_paragraph_with_no_list_id_passes_through_unchanged() {
        let content = "mi<tg<open-att__<paragraph-definition<align>left\nmi<mk<pard-end__\n";
        let out = make_lists(content, &[], MakeListsOptions::default());
        assert_eq!(out, content);
    }

    #[test]
    fn a_first_list_paragraph_opens_a_list_and_an_item() {
        let content = pard("1", "0", 240.0);
        let out = make_lists(content.as_str(), &[], MakeListsOptions::default());
        assert!(out.contains("mi<mk<list_start\n"), "{out}");
        assert!(out.contains("mi<tg<open-att__<list<list-id>1<level>0<list-type>not-defined\n"), "{out}");
        assert!(out.contains("mi<mk<item_start\n"), "{out}");
    }

    #[test]
    fn increasing_indent_with_the_same_list_id_nests_a_new_list() {
        let mut content = pard("1", "0", 240.0);
        content.push_str(&pard("1", "1", 480.0));
        let out = make_lists(content.as_str(), &[], MakeListsOptions::default());
        assert_eq!(out.matches("mi<mk<list_start\n").count(), 2, "{out}");
        // No list closed yet -- the second, deeper paragraph nests.
        assert_eq!(out.matches("mi<mk<list_close\n").count(), 0, "{out}");
    }

    #[test]
    fn same_indent_with_the_same_list_id_starts_a_new_item_not_a_new_list() {
        let mut content = pard("1", "0", 240.0);
        content.push_str(&pard("1", "0", 240.0));
        let out = make_lists(content.as_str(), &[], MakeListsOptions::default());
        assert_eq!(out.matches("mi<mk<list_start\n").count(), 1, "{out}");
        assert_eq!(out.matches("mi<mk<item_start\n").count(), 2, "{out}");
        assert_eq!(out.matches("mi<tg<item_end__\n").count(), 1, "{out}");
    }

    #[test]
    fn switching_list_id_closes_the_old_list_and_opens_a_new_one() {
        let mut content = pard("1", "0", 240.0);
        content.push_str(&pard("2", "0", 240.0));
        let out = make_lists(content.as_str(), &[], MakeListsOptions::default());
        assert_eq!(out.matches("mi<mk<list_start\n").count(), 2, "{out}");
        assert_eq!(out.matches("mi<mk<list_close\n").count(), 1, "{out}");
    }

    #[test]
    fn an_end_list_token_closes_every_open_list() {
        let mut content = pard("1", "0", 240.0);
        content.push_str(&pard("1", "1", 480.0));
        content.push_str("mi<mk<sect-start\n");
        let out = make_lists(content.as_str(), &[], MakeListsOptions::default());
        assert_eq!(out.matches("mi<mk<list_close\n").count(), 2, "{out}");
    }

    #[test]
    fn a_heading_style_paragraph_closes_open_lists_and_starts_none() {
        let mut opts = MakeListsOptions::default();
        opts.no_headings_as_list = true;
        let mut content = pard("1", "0", 240.0);
        content.push_str(
            "mi<mk<style-name<heading 1\nmi<tg<open-att__<paragraph-definition<list-id>1<list-level>0\nmi<mk<pard-end__\n",
        );
        let out = make_lists(content.as_str(), &[], opts);
        assert_eq!(out.matches("mi<mk<list_close\n").count(), 1, "{out}");
        assert_eq!(out.matches("mi<mk<list_start\n").count(), 1, "{out}");
    }

    #[test]
    fn list_type_resolves_to_unordered_for_a_bullet_level_and_ordered_otherwise() {
        let mut bullet_list = ListInfo::default();
        bullet_list.attributes.insert("list-table-id".to_string(), "100".to_string());
        bullet_list.list_id.push("1".to_string());
        let mut level0 = IndexMap::new();
        level0.insert("numbering-type".to_string(), "bullet".to_string());
        bullet_list.levels.push(level0);

        let out = make_lists(pard("1", "0", 240.0).as_str(), &[bullet_list], MakeListsOptions::default());
        assert!(out.contains("<list-type>unordered"), "{out}");

        let mut arabic_list = ListInfo::default();
        arabic_list.list_id.push("2".to_string());
        let mut level0b = IndexMap::new();
        level0b.insert("numbering-type".to_string(), "arabic".to_string());
        arabic_list.levels.push(level0b);
        let out2 = make_lists(pard("2", "0", 240.0).as_str(), &[arabic_list], MakeListsOptions::default());
        assert!(out2.contains("<list-type>ordered"), "{out2}");
    }

    #[test]
    fn write_list_info_appends_the_matched_lists_own_attributes() {
        let mut opts = MakeListsOptions::default();
        opts.write_list_info = true;
        let mut list = ListInfo::default();
        list.attributes.insert("list-hybrid".to_string(), "true".to_string());
        list.list_id.push("1".to_string());
        let mut level0 = IndexMap::new();
        level0.insert("numbering-type".to_string(), "bullet".to_string());
        list.levels.push(level0);

        let out = make_lists(pard("1", "0", 240.0).as_str(), &[list], opts);
        assert!(out.contains("<list-hybrid>true"), "{out}");
        assert!(out.contains("<numbering-type>bullet"), "{out}");
    }

    #[test]
    fn a_list_with_no_matching_id_falls_back_to_the_marker_derived_list_type() {
        let known = ListInfo::default();
        let mut content = "mi<mk<list-type_<item\n".to_string();
        content.push_str(&pard("999", "0", 240.0));
        let out = make_lists(content.as_str(), &[known], MakeListsOptions::default());
        assert!(out.contains("<list-type>unordered"), "{out}");
    }
}
