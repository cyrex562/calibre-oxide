//! Port of `old_src/src/calibre/ebooks/rtf2xml/override_table.py`
//! (`OverrideTable`).
//!
//! Parses a `\listoverridetable` block's `{\listoverride ...}` groups
//! into `<override-list>` tags, and -- the module's other job --
//! matches each override's `list-table-id` against
//! [`super::list_table::ListInfo`]'s own `list-table-id` attribute
//! (collected earlier by [`super::list_table::parse_list_table`]),
//! appending the override's `list-id` into the FIRST matching list's
//! [`ListInfo::list_id`] field. Python mutates `self.__list_of_lists`
//! (the same object `list_table.py` built and returned) in place and
//! returns it again; this port instead takes `&mut Vec<ListInfo>` and
//! mutates it directly, returning only the transformed
//! `<override-list>` string -- there's no need to hand the caller back
//! something it already holds a mutable reference to.
//!
//! # A `run_level`-independent crash, made unconditional rather than "fixed"
//!
//! `__parse_override_dict`'s very first check reads `self.__level`
//! (never assigned anywhere in this class -- confirmed by grep -- a
//! typo for `self.__run_level`, used correctly one line later).
//! Reading an unset attribute raises `AttributeError` in Python
//! *before* the `> 3` comparison ever runs, so this crashes whenever
//! an override has no `list-id` token, regardless of `run_level` --
//! unlike a properly gated check. [`OverrideTableError::MissingListId`]
//! is therefore raised unconditionally here too, matching what Python
//! actually (always) does on this path, rather than mirroring a
//! `run_level` gate that doesn't really exist.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use indexmap::IndexMap;
use thiserror::Error;

use super::list_table::ListInfo;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OverrideTableError {
    /// See this module's own doc for why this is unconditional rather
    /// than `run_level`-gated.
    #[error("This override does not appear to have a list-id\n")]
    MissingListId,
    /// Port of the second, correctly `self.__run_level`-gated check.
    #[error("This override does not appear to have a list-table-id\n")]
    MissingListTableId,
    /// Port of `__after_bracket_func`'s `run_level > 3`-gated raise.
    #[error("No matching token after open bracket\ntoken is \"{0}\n\"")]
    NoMatchingToken(String),
}

pub type Result<T> = std::result::Result<T, OverrideTableError>;

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn last_four(line: &str) -> String {
    if line.len() >= 4 { line[line.len() - 4..].to_string() } else { line.to_string() }
}

fn value_field(line: &str) -> &str {
    if line.len() >= 20 { &line[20..] } else { "" }
}

fn override_dict(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "cw<ls<lis-tbl-id" => "list-table-id",
        "cw<ls<list-id___" => "list-id",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Default,
    Override,
    UnsureOb,
}

struct OverrideTableBuilder<'a> {
    state: State,
    override_list: Vec<IndexMap<String, String>>,
    override_ob_count: String,
    list_of_lists: &'a mut Vec<ListInfo>,
}

impl OverrideTableBuilder<'_> {
    /// Port of `__after_bracket_func`.
    fn after_bracket(&mut self, line: &str, tok: &str, ob_count: &str, run_level: u32) -> Result<()> {
        if tok == "cw<ls<lis-overid" {
            self.state = State::Override;
            self.override_ob_count = ob_count.to_string();
            self.override_list.push(IndexMap::new());
        } else if run_level > 3 {
            return Err(OverrideTableError::NoMatchingToken(line.to_string()));
        }
        Ok(())
    }

    /// Port of `__override_func`.
    fn in_override(&mut self, line: &str, tok: &str, cb_count: &str, run_level: u32) -> Result<()> {
        if tok == "cb<nu<clos-brack" && cb_count == self.override_ob_count {
            self.state = State::Default;
            self.parse_override_dict(run_level)?;
        } else if let Some(att) = override_dict(tok) {
            let value = value_field(line).to_string();
            self.override_list.last_mut().expect("Override state implies a pushed entry").insert(att.to_string(), value);
        }
        Ok(())
    }

    /// Port of `__parse_override_dict`.
    fn parse_override_dict(&mut self, run_level: u32) -> Result<()> {
        let entry = self.override_list.last().expect("just closed a pushed override entry");
        let Some(list_id) = entry.get("list-id").cloned() else {
            return Err(OverrideTableError::MissingListId);
        };
        let current_table_id = entry.get("list-table-id").cloned();
        if current_table_id.is_none() && run_level > 3 {
            return Err(OverrideTableError::MissingListTableId);
        }

        for list in self.list_of_lists.iter_mut() {
            let old_table_id = list.attributes.get("list-table-id").cloned();
            if old_table_id == current_table_id {
                list.list_id.push(list_id);
                break;
            }
        }
        Ok(())
    }

    /// Port of `__write_final_string`.
    fn write_final_string(&self) -> String {
        let mut out = String::from("mi<mk<over_beg_\n");
        // Preserved upstream quirk (same shape as list_table.rs's own
        // -- see its module doc): the real `+=` re-reads its own old
        // value, so the `mi<mk<over_beg_` marker ends up duplicated.
        let prefix = out.clone();
        out.push_str(&format!("mi<tg<open______<override-table\nmi<mk<overbeg__\n{prefix}"));

        for entry in &self.override_list {
            out.push_str("mi<tg<empty-att_<override-list");
            for (k, v) in entry {
                out.push_str(&format!("<{k}>{v}"));
            }
            out.push('\n');
        }
        out.push('\n');
        out.push_str("mi<mk<overri-end\nmi<tg<close_____<override-table\n");
        out.push_str("mi<mk<overribend_\n");
        out
    }
}

/// Port of `OverrideTable.parse_override_table`. `list_of_lists`
/// should be [`super::list_table::parse_list_table`]'s own second
/// return value -- mutated in place (see this module's own doc for
/// why the return type doesn't hand it back).
pub fn parse_override_table(content: &str, list_of_lists: &mut Vec<ListInfo>, run_level: u32) -> Result<String> {
    let mut b = OverrideTableBuilder {
        state: State::Default,
        override_list: Vec::new(),
        override_ob_count: String::new(),
        list_of_lists,
    };
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
            State::Override => b.in_override(line, tok, &cb_count, run_level)?,
        }
    }
    Ok(b.write_final_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_with_table_id(id: &str) -> ListInfo {
        let mut list = ListInfo::default();
        list.attributes.insert("list-table-id".to_string(), id.to_string());
        list
    }

    #[test]
    fn an_override_appends_its_list_id_to_the_matching_list() {
        let mut lists = vec![list_with_table_id("100")];
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<100\n\
cw<ls<list-id___<nu<7\n\
cb<nu<clos-brack<0001\n";
        let out = parse_override_table(content, &mut lists, 1).unwrap();
        assert_eq!(lists[0].list_id, vec!["7".to_string()]);
        assert!(out.contains("mi<tg<empty-att_<override-list<list-table-id>100<list-id>7\n"), "{out}");
    }

    #[test]
    fn an_override_with_no_matching_list_table_id_is_dropped_silently() {
        let mut lists = vec![list_with_table_id("100")];
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<999\n\
cw<ls<list-id___<nu<7\n\
cb<nu<clos-brack<0001\n";
        parse_override_table(content, &mut lists, 1).unwrap();
        assert!(lists[0].list_id.is_empty());
    }

    #[test]
    fn only_the_first_matching_list_receives_the_override() {
        let mut lists = vec![list_with_table_id("100"), list_with_table_id("100")];
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<100\n\
cw<ls<list-id___<nu<7\n\
cb<nu<clos-brack<0001\n";
        parse_override_table(content, &mut lists, 1).unwrap();
        assert_eq!(lists[0].list_id, vec!["7".to_string()]);
        assert!(lists[1].list_id.is_empty());
    }

    #[test]
    fn multiple_overrides_produce_one_tag_each_in_order() {
        let mut lists = vec![list_with_table_id("100"), list_with_table_id("200")];
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<100\n\
cw<ls<list-id___<nu<7\n\
cb<nu<clos-brack<0001\n\
ob<nu<open-brack<0002\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<200\n\
cw<ls<list-id___<nu<8\n\
cb<nu<clos-brack<0002\n";
        let out = parse_override_table(content, &mut lists, 1).unwrap();
        assert_eq!(out.matches("mi<tg<empty-att_<override-list").count(), 2, "{out}");
        assert_eq!(lists[0].list_id, vec!["7".to_string()]);
        assert_eq!(lists[1].list_id, vec!["8".to_string()]);
    }

    #[test]
    fn an_override_missing_list_id_always_errors_regardless_of_run_level() {
        let mut lists: Vec<ListInfo> = Vec::new();
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<lis-tbl-id<nu<100\n\
cb<nu<clos-brack<0001\n";
        assert_eq!(parse_override_table(content, &mut lists, 1).unwrap_err(), OverrideTableError::MissingListId);
        assert_eq!(parse_override_table(content, &mut lists, 4).unwrap_err(), OverrideTableError::MissingListId);
    }

    #[test]
    fn missing_list_table_id_only_errors_at_high_run_level() {
        let mut lists: Vec<ListInfo> = Vec::new();
        let content = "\
ob<nu<open-brack<0001\n\
cw<ls<lis-overid<nu<true\n\
cw<ls<list-id___<nu<7\n\
cb<nu<clos-brack<0001\n";
        assert!(parse_override_table(content, &mut lists, 1).is_ok());
        assert_eq!(
            parse_override_table(content, &mut lists, 4).unwrap_err(),
            OverrideTableError::MissingListTableId
        );
    }

    #[test]
    fn an_unrecognized_token_after_a_bracket_is_ignored_at_low_run_level_and_errors_at_high() {
        let mut lists: Vec<ListInfo> = Vec::new();
        let content = "ob<nu<open-brack<0001\ncw<ls<unknown-tok<nu<true\n";
        assert!(parse_override_table(content, &mut lists, 1).is_ok());
        assert!(matches!(
            parse_override_table(content, &mut lists, 4),
            Err(OverrideTableError::NoMatchingToken(_))
        ));
    }
}
