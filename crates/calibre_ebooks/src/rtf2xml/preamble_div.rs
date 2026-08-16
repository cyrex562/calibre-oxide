//! Port of `old_src/src/calibre/ebooks/rtf2xml/preamble_div.py`
//! (`PreambleDiv`).
//!
//! Despite the name, this pass doesn't just "divide" the preamble in
//! the sense of splitting it up for later passes to read back in
//! order -- it's the pass that decides, token by token, when the
//! preamble *ends* and the body begins, and along the way it
//! classifies every top-level bracketed group hanging directly off the
//! document's outermost `{` (font table, color table, style sheet,
//! list table, override table, revision table, `\info` doc-info) into
//! its own accumulator string. Once the body boundary is found, all of
//! those accumulator strings (falling back to hard-coded defaults for
//! font/color/style if the document never defined its own) are written
//! out together, immediately followed by synthetic `doc`/`preamble`/
//! `body` open tags -- so everything this pass buffers only actually
//! appears in the output once, in a fixed order, right at that
//! boundary.
//!
//! Checkpoint `make_preamble_divisions` in `ParseRtf.py`; runs
//! immediately after `list_numbers.py`'s pass and before
//! `hex_2_utf8.py`'s preamble pass (both out of scope here).
//!
//! # Bracket numbering (why `cb_count == "0002"` means "back at depth 1")
//!
//! [`super::process_tokens`]'s `ob<nu<open-brack<NNNN` /
//! `cb<nu<clos-brack<NNNN` sequence number is a **nesting-depth**
//! counter (incremented on open, decremented after the matching
//! close), not a globally-unique running id -- so *every* group opened
//! directly under the single outermost `{` (bracket `0001`) is itself
//! numbered `0002`, whether it's the font table, the color table, the
//! style sheet, or any other depth-2 sibling group, each reusing
//! `0002` in turn as the previous sibling closes and the next opens.
//! That's what makes the repeated `if cb_count == "0002"` checks below
//! (in [`para_def_func`], [`text_func`], [`row_def_func`],
//! [`new_section_func`]) a reliable "we just closed *some* depth-2
//! preamble group and are back at depth 1" signal, regardless of which
//! group it was.
//!
//! # Scope boundary: `list_table.py` / `override_table.py` are not ported here
//!
//! The Python constructs a `list_table.ListTable` and (once a list
//! table is found) an `override_table.OverrideTable`, and calls their
//! `parse_list_table`/`parse_override_table` methods to transform the
//! raw accumulated `\listtable`/`\listoverridetable` group content into
//! tagged output and to populate `self.__all_lists` (returned by
//! `make_preamble_divisions` and threaded, much later in the real
//! pipeline, into the out-of-scope `make_lists.py`, issue #189). Both
//! `list_table.py` and `override_table.py` are explicitly out of scope
//! for this issue (see `crate::rtf2xml`'s module docs, "Not here"
//! section: `list_*` besides [`super::list_numbers`] is a follow-up
//! issue) and are not ported anywhere else in this crate yet. So
//! [`list_table_func`] and [`override_table_func`] below faithfully
//! reproduce only the *division* logic Python performs before handing
//! off to those parsers -- finding the group's boundaries and
//! accumulating its raw lines -- and, instead of calling the unported
//! parsers, pass that raw content straight through unchanged (rather
//! than the real transformed `mi<tg<...<list-table` tag shape) and
//! leave [`PreambleDivOutput::list_of_lists`] empty. This is a
//! deliberate, documented simplification, not a bug being preserved.

use indexmap::IndexMap;
use thiserror::Error;

/// Errors [`make_preamble_divisions`] can return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PreambleDivError {
    /// Port of a genuine upstream crash: `'PreambleDiv' object has no
    /// attribute '_PreambleDiv__ignore_num'` (`AttributeError`).
    ///
    /// `__found_font_table_func` transitions straight to state
    /// `'ignore'` when a *second* top-level `cw<it<font-table` group is
    /// found (`self.__found_font_table` already true from the first
    /// one) -- but, unlike `__font_table_func`'s own "nested bracket
    /// one level too deep" ignore path (which sets both
    /// `self.__ignore_num` and `self.__previous_state` before handing
    /// off), it sets neither. `__ignore_func` then unconditionally
    /// reads `self.__ignore_num` on the very next dispatched line,
    /// which raises if no earlier, unrelated use of the ignore
    /// mechanism happened to have set it first. Verified in isolation
    /// (no `calibre` import) with a throwaway class reproducing just
    /// this attribute-access shape; see
    /// `duplicate_font_table_crashes_with_unset_ignore_num` below for
    /// the fixture that reaches it through the full state machine.
    #[error(
        "'PreambleDiv' object has no attribute 'ignore_num' -- a duplicate top-level \
         font-table group was ignored via a path that never initializes it"
    )]
    IgnoreNumUnset,
    /// Port of the implicit `ValueError` from Python's `int(...)` on a
    /// bracket sequence number that isn't purely numeric. Never
    /// actually reached for well-formed [`super::process_tokens`]
    /// output (every real `ob_count`/`cb_count`/`close_group_count` is
    /// a 4-digit zero-padded number by construction), but kept as a
    /// `Result`-returning guard rather than a panic, matching this
    /// crate's convention elsewhere of not panicking on malformed
    /// input.
    #[error("invalid bracket sequence number: {0:?}")]
    InvalidBracketCount(String),
}

pub type Result<T> = std::result::Result<T, PreambleDivError>;

/// Result of [`make_preamble_divisions`]: the transformed content plus
/// the Python's `self.__all_lists` return value.
///
/// `list_of_lists` is always empty in this port -- see the module
/// docs' "Scope boundary" section for why: populating it for real
/// requires the unported `list_table.py`/`override_table.py`. The
/// field is kept (rather than dropped) because it's a genuine part of
/// this function's public API, even though nothing in this crate
/// consumes it yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreambleDivOutput {
    pub content: String,
    pub list_of_lists: Vec<IndexMap<String, String>>,
}

/// Port of `self.__state`'s possible values (`self.__state_dict`'s
/// keys), minus one dead entry: Python also registers a `'default'` ->
/// `__default_func` (no-op) pair in `__state_dict`, but `self.__state`
/// is never actually assigned the string `'default'` anywhere in the
/// class (the only ever-assigned states are `rtf_header`, `preamble`,
/// `font_table`, `color_table`, `style_sheet`, `list_table`,
/// `override_table`, `revision_table`, `doc_info`, `body`, and
/// `ignore` -- all reproduced as variants below). That dict entry is
/// dead code, intentionally not reproduced as a reachable variant
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    RtfHeader,
    Preamble,
    FontTable,
    ColorTable,
    StyleSheet,
    ListTable,
    OverrideTable,
    RevisionTable,
    DocInfo,
    Body,
    Ignore,
}

/// Port of `PreambleDiv`'s per-call instance state (`__initiate_values`
/// plus the fields set in `__init__`), threaded explicitly here instead
/// of as `self` fields.
struct State {
    state: Stage,
    /// Port of `self.__previous_state`, restored by `__ignore_func`.
    /// Only ever meaningfully read once [`Self::ignore_num`] is
    /// `Some`, since both are always set together (in
    /// [`font_table_func`]'s "nested one level too deep" branch) --
    /// see [`PreambleDivError::IgnoreNumUnset`]'s doc for the one path
    /// that reaches state `Ignore` *without* setting either. Defaults
    /// to `Preamble` arbitrarily; never read while meaningless.
    previous_state: Stage,
    /// Port of `self.__ignore_num`. Modeled as `Option` because the
    /// Python attribute is sometimes genuinely never assigned before
    /// being read -- see [`PreambleDivError::IgnoreNumUnset`].
    ignore_num: Option<String>,
    /// Port of `self.__ob_count`. Initial value `""` (Python's
    /// `self.__ob_count = ''`) is never read numerically in practice:
    /// every numeric use of it happens only on the very line whose
    /// token *is* `ob<nu<open-brack`, by which point the main loop has
    /// already just overwritten it from that line.
    ob_count: String,
    /// Port of `self.__cb_count`. Python re-uses this field for two
    /// different sentinel shapes: the true initial value is the empty
    /// string `''` (`__initiate_values`), but every `__found_*_func`
    /// resets it to the *integer* `0` before entering its table state
    /// -- and some of that state's close-check functions compare via
    /// `int(self.__cb_count) == int(self.__close_group_count)` while
    /// others compare via plain `==`. A single string sentinel `"0"`
    /// (used for the found-func reset) satisfies both: it never
    /// string-equals a real 4-digit `close_group_count`, and it parses
    /// to `0` exactly like Python's literal `int` `0` would.
    cb_count: String,
    /// Port of `self.__close_group_count`. Initial value `""` is never
    /// read before being overwritten by a `__found_*_func`.
    close_group_count: String,
    rtf_final: String,
    found_font_table: bool,
    font_table_final: String,
    color_table_final: String,
    style_sheet_final: String,
    list_table_final: String,
    override_table_final: String,
    revision_table_final: String,
    doc_info_table_final: String,
    individual_font: bool,
    old_font: bool,
    /// Port of `self.__page`, an `IndexMap` (not `HashMap`) because
    /// `__print_page_info` iterates it in dict order, and
    /// `__margin_func` can insert brand-new keys (`paper-width`/
    /// `paper-height`, not present among the five defaults) at
    /// whatever point they're first encountered in the document --
    /// Python dicts (and `IndexMap`) both append genuinely-new keys at
    /// the end while updating an existing key's value in place, so
    /// this ordering is semantically observable in the output.
    page: IndexMap<String, String>,
    all_lists: Vec<IndexMap<String, String>>,
    out: String,
    no_namespace: bool,
}

impl State {
    fn new(no_namespace: bool) -> Self {
        State {
            state: Stage::Preamble,
            previous_state: Stage::Preamble,
            ignore_num: None,
            ob_count: String::new(),
            cb_count: String::new(),
            close_group_count: String::new(),
            rtf_final: String::new(),
            found_font_table: false,
            font_table_final: String::new(),
            color_table_final: String::new(),
            style_sheet_final: String::new(),
            list_table_final: String::new(),
            override_table_final: String::new(),
            revision_table_final: String::new(),
            doc_info_table_final: String::new(),
            individual_font: false,
            old_font: false,
            page: default_page(),
            all_lists: Vec::new(),
            out: String::new(),
            no_namespace,
        }
    }

    /// Port of `__write_preamble`. Writes all the accumulated
    /// division strings, falling back to hard-coded defaults for
    /// font/color/style if the document never defined its own, then a
    /// fixed `doc`/`preamble`/`body` tag skeleton.
    ///
    /// Note what's deliberately *not* here: Python also defines
    /// `__section`/`__print_sec_info`/`__section_func` for tracking
    /// section (e.g. `\cols`) info, but the only line that would wire
    /// `__section_func` into `__state_dict` is commented out
    /// (`# 'cw<tb<columns___' : self.__section_func,`), and the only
    /// call to `__print_sec_info` inside `__write_preamble` is also
    /// commented out. Both are unreachable dead code in the Python;
    /// `self.__section` stays permanently empty and no
    /// `section-definition` tag is ever emitted. Not reproduced here.
    fn write_preamble(&mut self) {
        if self.no_namespace {
            self.out.push_str("mi<tg<open______<doc\n");
        } else {
            self.out
                .push_str("mi<tg<open-att__<doc<xmlns>http://rtf2xml.sourceforge.net/\n");
        }
        self.out.push_str("mi<tg<open______<preamble\n");
        self.out.push_str(&self.rtf_final);
        if self.color_table_final.is_empty() {
            self.color_table_final = make_default_color_table();
        }
        if self.font_table_final.is_empty() {
            self.font_table_final = make_default_font_table();
        }
        self.out.push_str(&self.font_table_final);
        self.out.push_str(&self.color_table_final);
        if self.style_sheet_final.is_empty() {
            self.style_sheet_final = make_default_style_table();
        }
        self.out.push_str(&self.style_sheet_final);
        self.out.push_str(&self.list_table_final);
        self.out.push_str(&self.override_table_final);
        self.out.push_str(&self.revision_table_final);
        self.out.push_str(&self.doc_info_table_final);
        self.print_page_info();
        self.out.push_str("ob<nu<open-brack<0001\n");
        self.out.push_str("ob<nu<open-brack<0002\n");
        self.out.push_str("cb<nu<clos-brack<0002\n");
        self.out.push_str("mi<tg<close_____<preamble\n");
        self.out.push_str("mi<tg<open______<body\n");
        self.out.push_str("mi<mk<body-open_\n");
    }

    /// Port of `__print_page_info`.
    fn print_page_info(&mut self) {
        self.out.push_str("mi<tg<empty-att_<page-definition");
        for (key, value) in &self.page {
            self.out.push_str(&format!("<{key}>{value}"));
        }
        self.out.push('\n');
    }
}

/// Port of `self.__page`'s literal initial dict, in source order.
fn default_page() -> IndexMap<String, String> {
    let mut page = IndexMap::new();
    page.insert("margin-top".to_string(), "72".to_string());
    page.insert("margin-bottom".to_string(), "72".to_string());
    page.insert("margin-left".to_string(), "90".to_string());
    page.insert("margin-right".to_string(), "90".to_string());
    page.insert("gutter".to_string(), "0".to_string());
    page
}

/// Port of `__make_default_font_table`.
fn make_default_font_table() -> String {
    "mi<tg<open______<font-table\n\
     mi<mk<fonttb-beg\n\
     mi<mk<fontit-beg\n\
     cw<ci<font-style<nu<0\n\
     tx<nu<__________<Times;\n\
     mi<mk<fontit-end\n\
     mi<mk<fonttb-end\n\
     mi<tg<close_____<font-table\n"
        .to_string()
}

/// Port of `__make_default_color_table`.
fn make_default_color_table() -> String {
    "mi<tg<open______<color-table\n\
     mi<mk<clrtbl-beg\n\
     cw<ci<red_______<nu<00\n\
     cw<ci<green_____<nu<00\n\
     cw<ci<blue______<en<00\n\
     mi<mk<clrtbl-end\n\
     mi<tg<close_____<color-table\n"
        .to_string()
}

/// Port of `__make_default_style_table`.
fn make_default_style_table() -> String {
    "mi<tg<open______<style-table\n\
     mi<mk<styles-beg\n\
     mi<mk<stylei-beg\n\
     cw<ci<font-style<nu<0\n\
     tx<nu<__________<Normal;\n\
     mi<mk<stylei-end\n\
     mi<mk<stylei-beg\n\
     cw<ss<char-style<nu<0\n\
     tx<nu<__________<Default Paragraph Font;\n\
     mi<mk<stylei-end\n\
     mi<mk<styles-end\n\
     mi<tg<close_____<style-table\n"
        .to_string()
}

/// Port of Python `str` slicing (`s[start:end]`), supporting negative
/// indices counted from the end, and clamping out-of-range indices
/// instead of panicking -- exactly Python's own slice semantics.
/// Assumes ASCII input, true for every fixed-prefix line in this
/// intermediate format.
fn python_slice(s: &str, start: isize, end: isize) -> &str {
    let len = s.len() as isize;
    let norm = |i: isize| -> usize {
        let v = if i < 0 { (len + i).max(0) } else { i.min(len) };
        v as usize
    };
    let start = norm(start);
    let end = norm(end).max(start);
    &s[start..end]
}

/// Port of `line[:16]` (`self.__token_info`).
fn token_info(line: &str) -> &str {
    python_slice(line, 0, 16)
}

/// Port of `line[-5:-1]`, used for `self.__ob_count`/`self.__cb_count`.
fn last_four(line: &str) -> &str {
    python_slice(line, -5, -1)
}

/// Port of `__margin_dict.get(line[6:16])`.
fn margin_key(info: &str) -> Option<&'static str> {
    match info {
        "margin-lef" => Some("margin-left"),
        "margin-rig" => Some("margin-right"),
        "margin-top" => Some("margin-top"),
        "margin-bot" => Some("margin-bottom"),
        "gutter____" => Some("gutter"),
        "paper-widt" => Some("paper-width"),
        "paper-hght" => Some("paper-height"),
        _ => None,
    }
}

/// Port of `int(...)` applied to a bracket sequence number.
fn parse_count(s: &str) -> Result<i64> {
    s.parse::<i64>()
        .map_err(|_| PreambleDivError::InvalidBracketCount(s.to_string()))
}

/// Port of `__ignore_func`.
fn ignore_func(st: &mut State) -> Result<()> {
    let ignore_num = st
        .ignore_num
        .as_ref()
        .ok_or(PreambleDivError::IgnoreNumUnset)?;
    if *ignore_num == st.cb_count {
        st.state = st.previous_state;
    }
    Ok(())
}

/// Port of `__found_rtf_head_func`.
fn found_rtf_head_func(st: &mut State) {
    st.state = Stage::RtfHeader;
}

/// Port of `__rtf_head_func`.
fn rtf_head_func(st: &mut State, line: &str, token: &str) {
    if st.ob_count == "0002" {
        let body = std::mem::take(&mut st.rtf_final);
        st.rtf_final = format!("mi<mk<rtfhed-beg\n{body}mi<mk<rtfhed-end\n");
        st.state = Stage::Preamble;
    } else if token == "tx<nu<__________" || token == "cw<pf<par-def___" {
        st.state = Stage::Body;
        let body = std::mem::take(&mut st.rtf_final);
        st.rtf_final = format!("mi<mk<rtfhed-beg\n{body}mi<mk<rtfhed-end\n");
        st.font_table_final = make_default_font_table();
        st.write_preamble();
        st.out.push_str(line);
    } else {
        st.rtf_final.push_str(line);
    }
}

/// Port of `__found_font_table_func`.
fn found_font_table_func(st: &mut State) {
    if st.found_font_table {
        // Duplicate top-level font-table group -- see
        // `PreambleDivError::IgnoreNumUnset` for the resulting crash
        // once `__ignore_func` runs (neither `ignore_num` nor
        // `previous_state` are set on this path).
        st.state = Stage::Ignore;
    } else {
        st.state = Stage::FontTable;
        st.font_table_final.clear();
    }
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
    st.found_font_table = true;
}

/// Port of `__font_table_func`.
fn font_table_func(st: &mut State, line: &str, token: &str) -> Result<()> {
    if st.cb_count == st.close_group_count {
        st.state = Stage::Preamble;
        let body = std::mem::take(&mut st.font_table_final);
        st.font_table_final =
            format!("mi<tg<open______<font-table\nmi<mk<fonttb-beg\n{body}mi<mk<fonttb-end\nmi<tg<close_____<font-table\n");
    } else if token == "ob<nu<open-brack" {
        if parse_count(&st.ob_count)? == parse_count(&st.close_group_count)? + 1 {
            st.font_table_final.push_str("mi<mk<fontit-beg\n");
            st.individual_font = true;
        } else {
            st.previous_state = Stage::FontTable;
            st.state = Stage::Ignore;
            st.ignore_num = Some(st.ob_count.clone());
        }
    } else if token == "cb<nu<clos-brack" {
        if parse_count(&st.cb_count)? == parse_count(&st.close_group_count)? + 1 {
            st.individual_font = false;
            st.font_table_final.push_str("mi<mk<fontit-end\n");
        }
    } else if st.individual_font {
        if st.old_font && token == "tx<nu<__________" {
            if line.contains(';') {
                st.font_table_final.push_str(line);
                st.font_table_final.push_str("mi<mk<fontit-end\n");
                st.individual_font = false;
            }
        } else {
            st.font_table_final.push_str(line);
        }
    } else if token == "cw<ci<font-style" {
        st.old_font = true;
        st.individual_font = true;
        st.font_table_final.push_str("mi<mk<fontit-beg\n");
        st.font_table_final.push_str(line);
    }
    Ok(())
}

/// Port of `__found_color_table_func`.
fn found_color_table_func(st: &mut State) {
    st.state = Stage::ColorTable;
    st.color_table_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__color_table_func`.
fn color_table_func(st: &mut State, line: &str) -> Result<()> {
    if parse_count(&st.cb_count)? == parse_count(&st.close_group_count)? {
        st.state = Stage::Preamble;
        let body = std::mem::take(&mut st.color_table_final);
        st.color_table_final =
            format!("mi<tg<open______<color-table\nmi<mk<clrtbl-beg\n{body}mi<mk<clrtbl-end\nmi<tg<close_____<color-table\n");
    } else {
        st.color_table_final.push_str(line);
    }
    Ok(())
}

/// Port of `__found_style_sheet_func`.
fn found_style_sheet_func(st: &mut State) {
    st.state = Stage::StyleSheet;
    st.style_sheet_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__style_sheet_func`.
fn style_sheet_func(st: &mut State, line: &str, token: &str) -> Result<()> {
    if st.cb_count == st.close_group_count {
        st.state = Stage::Preamble;
        let body = std::mem::take(&mut st.style_sheet_final);
        st.style_sheet_final =
            format!("mi<tg<open______<style-table\nmi<mk<styles-beg\n{body}mi<mk<styles-end\nmi<tg<close_____<style-table\n");
    } else if token == "ob<nu<open-brack" {
        if parse_count(&st.ob_count)? == parse_count(&st.close_group_count)? + 1 {
            st.style_sheet_final.push_str("mi<mk<stylei-beg\n");
        }
    } else if token == "cb<nu<clos-brack" {
        if parse_count(&st.cb_count)? == parse_count(&st.close_group_count)? + 1 {
            st.style_sheet_final.push_str("mi<mk<stylei-end\n");
        }
    } else {
        st.style_sheet_final.push_str(line);
    }
    Ok(())
}

/// Port of `__found_list_table_func`.
fn found_list_table_func(st: &mut State) {
    st.state = Stage::ListTable;
    st.list_table_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__list_table_func`, minus the call into the unported
/// `list_table.ListTable.parse_list_table` -- see the module docs'
/// "Scope boundary" section. The raw accumulated group content is
/// passed through unchanged instead of the real transformed tags.
fn list_table_func(st: &mut State, line: &str, token: &str) {
    if st.cb_count == st.close_group_count {
        st.state = Stage::Preamble;
    } else if token.is_empty() {
        // Port of `elif self.__token_info == '': pass` -- guards the
        // synthetic empty-line dispatch `make_preamble_divisions`
        // performs once at true EOF (see its doc comment).
    } else {
        st.list_table_final.push_str(line);
    }
}

/// Port of `__found_override_table_func`, minus constructing the
/// unported `override_table.OverrideTable(list_of_lists=...)`.
fn found_override_table_func(st: &mut State) {
    st.state = Stage::OverrideTable;
    st.override_table_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__override_table_func` -- see [`list_table_func`]'s doc for
/// why this passes raw content through instead of transforming it.
fn override_table_func(st: &mut State, line: &str, token: &str) {
    if st.cb_count == st.close_group_count {
        st.state = Stage::Preamble;
    } else if token.is_empty() {
        // See the matching comment in `list_table_func`.
    } else {
        st.override_table_final.push_str(line);
    }
}

/// Port of `__found_revision_table_func`.
fn found_revision_table_func(st: &mut State) {
    st.state = Stage::RevisionTable;
    st.revision_table_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__revision_table_func`.
fn revision_table_func(st: &mut State, line: &str) -> Result<()> {
    if parse_count(&st.cb_count)? == parse_count(&st.close_group_count)? {
        st.state = Stage::Preamble;
        let body = std::mem::take(&mut st.revision_table_final);
        st.revision_table_final =
            format!("mi<tg<open______<revision-table\nmi<mk<revtbl-beg\n{body}mi<mk<revtbl-end\nmi<tg<close_____<revision-table\n");
    } else {
        st.revision_table_final.push_str(line);
    }
    Ok(())
}

/// Port of `__found_doc_info_func`.
fn found_doc_info_func(st: &mut State) {
    st.state = Stage::DocInfo;
    st.doc_info_table_final.clear();
    st.close_group_count = st.ob_count.clone();
    st.cb_count = "0".to_string();
}

/// Port of `__doc_info_func`.
fn doc_info_func(st: &mut State, line: &str, token: &str) -> Result<()> {
    if st.cb_count == st.close_group_count {
        st.state = Stage::Preamble;
        let body = std::mem::take(&mut st.doc_info_table_final);
        st.doc_info_table_final =
            format!("mi<tg<open______<doc-information\nmi<mk<doc-in-beg\n{body}mi<mk<doc-in-end\nmi<tg<close_____<doc-information\n");
    } else if token == "ob<nu<open-brack" {
        if parse_count(&st.ob_count)? == parse_count(&st.close_group_count)? + 1 {
            st.doc_info_table_final.push_str("mi<mk<docinf-beg\n");
        }
    } else if token == "cb<nu<clos-brack" {
        if parse_count(&st.cb_count)? == parse_count(&st.close_group_count)? + 1 {
            st.doc_info_table_final.push_str("mi<mk<docinf-end\n");
        }
    } else {
        st.doc_info_table_final.push_str(line);
    }
    Ok(())
}

/// Port of `__margin_func`.
fn margin_func(st: &mut State, line: &str) {
    let info = python_slice(line, 6, 16);
    match margin_key(info) {
        None => println!("woops!"), // port of Python's `print('woops!')` (stdout, not stderr)
        Some(key) => {
            let value = python_slice(line, 20, -1);
            st.page.insert(key.to_string(), value.to_string());
        }
    }
}

/// Port of `__body_func`.
fn body_func(st: &mut State, line: &str) {
    st.out.push_str(line);
}

/// Shared body of `__para_def_func` and `__row_def_func`, which are
/// identical in the Python (both check `cb_count == '0002'`, close the
/// preamble if so, then unconditionally write the triggering line).
fn close_preamble_then_write(st: &mut State, line: &str) {
    if st.cb_count == "0002" {
        st.state = Stage::Body;
        st.write_preamble();
    }
    st.out.push_str(line);
}

/// Port of `__para_def_func`.
fn para_def_func(st: &mut State, line: &str) {
    close_preamble_then_write(st, line);
}

/// Port of `__row_def_func`.
fn row_def_func(st: &mut State, line: &str) {
    close_preamble_then_write(st, line);
}

/// Port of `__text_func`.
fn text_func(st: &mut State, line: &str) {
    let cb_count: &str = if st.cb_count.is_empty() {
        "0002"
    } else {
        &st.cb_count
    };
    if cb_count == "0002" {
        st.state = Stage::Body;
        st.write_preamble();
    }
    st.out.push_str(line);
}

/// Port of `__new_section_func`.
fn new_section_func(st: &mut State, line: &str) {
    if st.cb_count == "0002" {
        st.state = Stage::Body;
        st.write_preamble();
    } else {
        // Port of Python's non-fatal `sys.stderr.write(...)` diagnostic
        // (not a raise) -- see `check_encoding.rs`'s precedent for
        // using `eprintln!` here. Note: even in this branch, Python
        // still writes `line` unconditionally afterward, *without*
        // ever having called `__write_preamble()` -- a genuine
        // malformed-output bug for this edge case, preserved as-is.
        eprintln!("module is preamble_div");
        eprintln!("method is __new_section_func");
        eprintln!("bracket count should be 2?");
    }
    st.out.push_str(line);
}

/// Port of `__preamble_func`'s inner dispatch: while in state
/// `'preamble'`, Python looks `self.__token_info` back up in the very
/// same `__state_dict` used for the outer (state-based) dispatch --
/// the dict does double duty, keyed partly by state name and partly by
/// specific trigger tokens.
fn preamble_func(st: &mut State, line: &str, token: &str) {
    match token {
        "cw<ri<rtf_______" => found_rtf_head_func(st),
        "cw<pf<par-def___" => para_def_func(st, line),
        "tx<nu<__________" => text_func(st, line),
        "cw<tb<row-def___" => row_def_func(st, line),
        "cw<sc<section___" | "cw<sc<sect-defin" => new_section_func(st, line),
        "cw<it<font-table" => found_font_table_func(st),
        "cw<it<colr-table" => found_color_table_func(st),
        "cw<ss<style-shet" => found_style_sheet_func(st),
        "cw<it<listtable_" => found_list_table_func(st),
        "cw<it<lovr-table" => found_override_table_func(st),
        "cw<it<revi-table" => found_revision_table_func(st),
        "cw<di<doc-info__" => found_doc_info_func(st),
        "cw<pa<margin-lef" | "cw<pa<margin-rig" | "cw<pa<margin-top" | "cw<pa<margin-bot"
        | "cw<pa<gutter____" | "cw<pa<paper-widt" | "cw<pa<paper-hght" => margin_func(st, line),
        // Anything else (including bare `ob<nu<open-brack` /
        // `cb<nu<clos-brack` lines for brackets not specifically
        // matched above -- e.g. the document's own outermost braces)
        // is silently dropped, matching Python's `if action: action(line)`
        // no-op when the token isn't in the dict.
        _ => {}
    }
}

/// Port of the main `while` loop's outer, state-based dispatch
/// (`self.__state_dict.get(self.__state)`).
fn dispatch(st: &mut State, line: &str, token: &str) -> Result<()> {
    match st.state {
        Stage::RtfHeader => rtf_head_func(st, line, token),
        Stage::Preamble => preamble_func(st, line, token),
        Stage::FontTable => font_table_func(st, line, token)?,
        Stage::ColorTable => color_table_func(st, line)?,
        Stage::StyleSheet => style_sheet_func(st, line, token)?,
        Stage::ListTable => list_table_func(st, line, token),
        Stage::OverrideTable => override_table_func(st, line, token),
        Stage::RevisionTable => revision_table_func(st, line)?,
        Stage::DocInfo => doc_info_func(st, line, token)?,
        Stage::Body => body_func(st, line),
        Stage::Ignore => ignore_func(st)?,
    }
    Ok(())
}

/// Port of `PreambleDiv.make_preamble_divisions`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file.
///
/// `no_namespace` mirrors the constructor's `no_namespace` flag
/// (selects between the two possible `doc` open tags). The
/// constructor's `run_level` is *not* threaded through: nothing in
/// `PreambleDiv` itself is `run_level`-gated (no `raise` of any kind
/// appears in the Python source) -- it's only forwarded to the
/// (out-of-scope, unported) `list_table.ListTable`/
/// `override_table.OverrideTable` constructors, so there is nothing
/// for this port to gate on yet.
pub fn make_preamble_divisions(content: &str, no_namespace: bool) -> Result<PreambleDivOutput> {
    let mut st = State::new(no_namespace);

    // Every real line keeps its trailing `\n` re-attached (mirroring
    // Python's `readline()`, which never strips it) so that every
    // `+= line` / `write(line)` call site above can append `line`
    // directly, matching the Python textually. One final, genuinely
    // empty `""` entry is appended after all real lines: Python's
    // `while line_to_read: line_to_read = read_obj.readline(); ...;
    // action(line)` loop reads and *dispatches* the empty string
    // `readline()` returns at true EOF once before the `while`
    // condition (checked only at the top of the next iteration) sees
    // it and stops -- so there's always one extra dispatch with
    // `line == ''` (no trailing newline at all, unlike every real
    // line) right before the pass ends. `list_table_func`'s and
    // `override_table_func`'s own `elif self.__token_info == '':
    // pass` guards only make sense in light of this.
    let mut lines: Vec<String> = content.lines().map(|l| format!("{l}\n")).collect();
    lines.push(String::new());

    for line in &lines {
        let token = token_info(line);
        if token == "ob<nu<open-brack" {
            st.ob_count = last_four(line).to_string();
        }
        if token == "cb<nu<clos-brack" {
            st.cb_count = last_four(line).to_string();
        }
        // Python's main loop also has `action = get(state); if action
        // is None: print(state); action(line)` -- the `action(line)`
        // is unconditional even when `action is None`, which would
        // raise `TypeError: 'NoneType' object is not callable`. This
        // is structurally unreachable in practice (every string ever
        // assigned to `self.__state` has a matching `__state_dict`
        // entry, verified by inspection of every `self.__state = `
        // assignment site), so Rust's exhaustive `match` in
        // `dispatch` -- which the compiler guarantees covers every
        // `Stage` variant -- is the natural, safe equivalent rather
        // than a reproduced dead crash path.
        dispatch(&mut st, line, token)?;
    }

    Ok(PreambleDivOutput {
        content: st.out,
        list_of_lists: st.all_lists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(n: u32) -> String {
        format!("ob<nu<open-brack<{n:04}")
    }
    fn cb(n: u32) -> String {
        format!("cb<nu<clos-brack<{n:04}")
    }
    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn rtf_header_closes_on_second_bracket() {
        // {\rtf1 {\fonttbl {\f0 Times;}}}
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            "cw<ci<font-style<nu<0",
            "tx<nu<__________<Times;",
            &cb(2),
            "cw<pf<par-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains("mi<mk<rtfhed-beg\nmi<mk<rtfhed-end\n"));
        // rtf1 marker itself is consumed by `found_rtf_head_func`, not
        // accumulated into `rtf_final`, so the wrapped header is empty.
        assert!(out.list_of_lists.is_empty());
    }

    #[test]
    fn missing_rtf_header_falls_through_to_body_on_par_def() {
        // No `{\rtf1...}` marker at all; a par-def control word while
        // still nominally in `rtf_header` state falls through directly
        // to body, synthesizing a default font table along the way.
        let content = lines(&["cw<ri<rtf_______<nu<1", "cw<pf<par-def___<nu<1"]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains("tx<nu<__________<Times;")); // default font table
        assert!(out.content.contains("mi<tg<open______<body\n"));
        // The triggering par-def line itself is written after the
        // preamble skeleton.
        assert!(out.content.ends_with("cw<pf<par-def___<nu<1\n"));
    }

    #[test]
    fn font_table_is_extracted_and_wrapped() {
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            "cw<ci<font-style<nu<0",
            "tx<nu<__________<Arial;",
            &cb(2),
            "cw<pf<par-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains(
            "mi<tg<open______<font-table\n\
             mi<mk<fonttb-beg\n\
             mi<mk<fontit-beg\n\
             cw<ci<font-style<nu<0\n\
             tx<nu<__________<Arial;\n\
             mi<mk<fontit-end\n\
             mi<mk<fonttb-end\n\
             mi<tg<close_____<font-table\n"
        ));
        // A real font table was found, so no default is synthesized.
        assert!(!out.content.contains("Times;"));
    }

    #[test]
    fn nested_too_deep_font_group_is_ignored_and_restored() {
        // {\rtf1 {\fonttbl {\f0{extra too-deep group}Times;}}}
        // depth: 1=outer, 2=fonttbl, 3=the individual font's own
        // group, 4=an unexpected nested group inside that.
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            &ob(3),
            "cw<ci<font-style<nu<0",
            &ob(4),
            "tx<nu<__________<dropped",
            &cb(4),
            "tx<nu<__________<Times;",
            &cb(3),
            &cb(2),
            "cw<pf<par-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        // The too-deep group's content never reaches font_table_final.
        assert!(!out.content.contains("dropped"));
        // Everything else in the individual font's own group (both
        // before and after the too-deep nested group) survives.
        assert!(out.content.contains("cw<ci<font-style<nu<0"));
        assert!(out.content.contains("Times;"));
    }

    #[test]
    fn duplicate_font_table_crashes_with_unset_ignore_num() {
        // Two sibling top-level `{\fonttbl ...}` groups; the second
        // one is routed straight to the `ignore` state without ever
        // setting `ignore_num` -- see `PreambleDivError::IgnoreNumUnset`.
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            "cw<ci<font-style<nu<0",
            "tx<nu<__________<Times;",
            &cb(2),
            &ob(2), // second, sibling font-table group reuses depth "0002"
            "cw<it<font-table<nu<1",
            &cb(2),
        ]);
        let err = make_preamble_divisions(&content, false).unwrap_err();
        assert_eq!(err, PreambleDivError::IgnoreNumUnset);
    }

    #[test]
    fn color_table_default_when_absent() {
        let content = lines(&["cw<ri<rtf_______<nu<1", "tx<nu<__________<hello"]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains(
            "mi<tg<open______<color-table\n\
             mi<mk<clrtbl-beg\n\
             cw<ci<red_______<nu<00\n\
             cw<ci<green_____<nu<00\n\
             cw<ci<blue______<en<00\n\
             mi<mk<clrtbl-end\n\
             mi<tg<close_____<color-table\n"
        ));
    }

    #[test]
    fn style_sheet_default_when_absent() {
        let content = lines(&["cw<ri<rtf_______<nu<1", "tx<nu<__________<hello"]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains("tx<nu<__________<Normal;"));
        assert!(out
            .content
            .contains("tx<nu<__________<Default Paragraph Font;"));
    }

    #[test]
    fn list_table_and_override_table_pass_through_raw_content() {
        // {\rtf1 {\fonttbl...} {\*\listtable ...}}
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            "cw<ci<font-style<nu<0",
            "tx<nu<__________<Times;",
            &cb(2),
            &ob(2),
            "cw<it<listtable_<nu<1",
            "cw<ls<list-hybri<nu<1",
            &cb(2),
            "cw<pf<par-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        // Raw accumulated content, unwrapped (see module docs' "Scope
        // boundary" section), still appears in the output.
        assert!(out.content.contains("cw<ls<list-hybri<nu<1"));
        assert!(out.list_of_lists.is_empty());
    }

    #[test]
    fn margin_and_paper_size_update_page_dict_in_insertion_order() {
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            "cw<ci<font-style<nu<0",
            "tx<nu<__________<Times;",
            &cb(2),
            "cw<pa<margin-lef<nu<1440",
            "cw<pa<paper-widt<nu<12240",
            "cw<pf<par-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        // Existing key (margin-left) updated in place, mid-sequence;
        // brand-new key (paper-width) appended at the end.
        let expected_page_info = "mi<tg<empty-att_<page-definition\
<margin-top>72<margin-bottom>72<margin-left>1440<margin-right>90<gutter>0<paper-width>12240\n";
        assert!(out.content.contains(expected_page_info));
    }

    #[test]
    fn unrecognized_margin_token_prints_woops_and_page_unchanged() {
        // margin_func is only ever dispatched (via `preamble_func`)
        // for one of the seven fixed margin tokens, whose `line[6:16]`
        // slice is by construction always a valid `margin_dict` key --
        // so the `None` "woops!" branch is unreachable through the
        // public state machine. Exercised directly here to document
        // the fallback exists and is harmless.
        let mut st = State::new(false);
        let before = st.page.clone();
        margin_func(&mut st, "cw<pa<xxxxxxxxxx<nu<1\n");
        assert_eq!(st.page, before);
    }

    #[test]
    fn row_def_ends_preamble_when_back_at_depth_one() {
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            &cb(2),
            "cw<tb<row-def___<nu<1",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        assert!(out.content.contains("mi<tg<open______<body\n"));
        assert!(out.content.contains("cw<tb<row-def___<nu<1\n"));
    }

    #[test]
    fn text_with_no_prior_bracket_defaults_to_body_transition() {
        // Old-style RTF with no brackets seen at all before body text:
        // `__text_func`'s `if self.__cb_count == '': cb_count = '0002'`
        // fallback treats this as if we were already back at depth 1.
        let content = "tx<nu<__________<hello\n";
        let out = make_preamble_divisions(content, false).unwrap();
        assert!(out.content.contains("mi<tg<open______<body\n"));
        assert!(out.content.ends_with("tx<nu<__________<hello\n"));
    }

    #[test]
    fn new_section_at_wrong_depth_writes_line_without_preamble_wrapper() {
        // A section-start token while cb_count != "0002" (here, still
        // the absolute-initial `""` sentinel, since state starts out
        // as `Preamble` already and no bracket has been seen yet):
        // Python writes three stderr diagnostics but does NOT call
        // `__write_preamble()` before still unconditionally writing
        // the triggering line -- a genuine malformed-output bug,
        // preserved as-is.
        let content = "cw<sc<section___<nu<1\n";
        let out = make_preamble_divisions(content, false).unwrap();
        assert!(!out.content.contains("mi<tg<open______<preamble\n"));
        assert_eq!(out.content, "cw<sc<section___<nu<1\n");
    }

    #[test]
    fn no_namespace_flag_changes_doc_open_tag() {
        let content = "tx<nu<__________<hello\n";
        let with_ns = make_preamble_divisions(content, false).unwrap();
        let without_ns = make_preamble_divisions(content, true).unwrap();
        assert!(with_ns
            .content
            .starts_with("mi<tg<open-att__<doc<xmlns>http://rtf2xml.sourceforge.net/\n"));
        assert!(without_ns.content.starts_with("mi<tg<open______<doc\n"));
    }

    #[test]
    fn synthetic_eof_dispatch_is_a_no_op_once_body_is_reached() {
        // A well-formed document that already reached `body` well
        // before EOF: the extra synthetic empty-line dispatch
        // (`body_func` writing `""`) must not perturb the output.
        let content = lines(&[
            &ob(1),
            "cw<ri<rtf_______<nu<1",
            &ob(2),
            "cw<it<font-table<nu<1",
            &cb(2),
            "cw<pf<par-def___<nu<1",
            "tx<nu<__________<hi",
            &cb(1),
        ]);
        let out = make_preamble_divisions(&content, false).unwrap();
        let expected_tail = format!("{}\n", cb(1));
        assert!(out.content.ends_with(&expected_tail));
        // The synthetic final empty-line dispatch must not add a stray
        // trailing blank line after the real content.
        assert!(!out.content.ends_with(&format!("{expected_tail}\n")));
    }

    #[test]
    fn empty_input_produces_default_preamble_and_no_crash() {
        let out = make_preamble_divisions("", false).unwrap();
        assert_eq!(out.content, "");
        assert!(out.list_of_lists.is_empty());
    }
}
