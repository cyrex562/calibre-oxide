//! Port of `old_src/src/calibre/ebooks/rtf2xml/paragraph_def.py`
//! (`ParagraphDef`).
//!
//! Assembles the RTF tokens that describe a paragraph's formatting
//! (alignment, indents, tabs, borders, the "which stylesheet style does
//! this paragraph use" reference, ...) into a single
//! `paragraph-definition` open/close tag pair wrapped around the
//! paragraph's own `mi<mk<para-start`/`mi<mk<para-end__` markers, and
//! assigns each distinct combination of attributes a stable
//! `style-number` (`sNNNN`) -- deduplicated by attribute-signature, not
//! by RTF's own `\s`/`\cs` stylesheet reference, since direct-formatted
//! paragraphs with no named style still need a number.
//!
//! Checkpoint `paragraph_def_info`; runs after [`super::paragraphs`]'s
//! pass. Returns the same "body style strings" (`list_of_styles` in
//! `ParseRtf.py`) that [`super::body_styles::insert_info`] consumes --
//! a real, in-scope dependency between the two, wired here as
//! [`ParagraphDefOutput::body_style_strings`].
//!
//! # `border_parse.py`: a genuine, private dependency
//!
//! `ParagraphDef` constructs a `border_parse.BorderParse()` and uses it
//! to turn `cw<bd<...` lines into attribute dictionary entries.
//! `border_parse.py` itself is not one of this issue's 18 files (its
//! only other consumer, `group_borders.py`, belongs to a later
//! follow-up issue), so rather than either gapping this out or adding
//! an extra top-level `pub mod` outside this issue's contract, its
//! `parse_border` logic is ported as a private, non-`pub` helper below,
//! scoped to this module's own use. (`styles.rs`, ported independently
//! in this same issue, needed the identical dependency for its own
//! `\stylesheet` border parsing and made the same call -- the two
//! private copies are intentionally not unified into a shared module,
//! to keep each pass's dependency footprint self-contained.)

use std::collections::BTreeMap;

use thiserror::Error;

/// Port of the `run_level > 3` gated `raise self.__bug_handler(msg)`
/// calls reachable from `ParagraphDef`'s methods.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParagraphDefError {
    /// Port of `f'no entry for {self.__token_info}\n'`, raised by
    /// `__tab_type_func`/`__tab_leader_func` when a tab-related token
    /// isn't in the (small, always-total-in-practice) tab-type
    /// dictionary.
    #[error("no entry for {0}\n")]
    NoTabEntry(String),
}

pub type Result<T> = std::result::Result<T, ParagraphDefError>;

// ---------------------------------------------------------------------
// Private port of border_parse.py's BorderParse.parse_border
// ---------------------------------------------------------------------

/// Port of `BorderParse.__border_dict`.
fn border_dict(key: &str) -> Option<&'static str> {
    Some(match key {
        "bor-t-r-hi" => "border-table-row-horizontal-inside",
        "bor-t-r-vi" => "border-table-row-vertical-inside",
        "bor-t-r-to" => "border-table-row-top",
        "bor-t-r-le" => "border-table-row-left",
        "bor-t-r-bo" => "border-table-row-bottom",
        "bor-t-r-ri" => "border-table-row-right",
        "bor-cel-bo" => "border-cell-bottom",
        "bor-cel-to" => "border-cell-top",
        "bor-cel-le" => "border-cell-left",
        "bor-cel-ri" => "border-cell-right",
        "bor-par-bo" => "border-paragraph-bottom",
        "bor-par-to" => "border-paragraph-top",
        "bor-par-le" => "border-paragraph-left",
        "bor-par-ri" => "border-paragraph-right",
        "bor-par-bx" => "border-paragraph-box",
        "bor-for-ev" => "border-for-every-paragraph",
        "bor-outsid" => "border-outside",
        "bor-none__" => "border",
        "bdr-li-wid" => "line-width",
        "bdr-sp-wid" => "padding",
        "bdr-color_" => "color",
        _ => return None,
    })
}

/// Port of `BorderParse.__border_style_dict`.
fn border_style_dict(key: &str) -> Option<&'static str> {
    Some(match key {
        "bdr-single" => "single",
        "bdr-doubtb" => "double-thickness-border",
        "bdr-shadow" => "shadowed-border",
        "bdr-double" => "double-border",
        "bdr-dotted" => "dotted-border",
        "bdr-dashed" => "dashed",
        "bdr-hair__" => "hairline",
        "bdr-inset_" => "inset",
        "bdr-das-sm" => "dash-small",
        "bdr-dot-sm" => "dot-dash",
        "bdr-dot-do" => "dot-dot-dash",
        "bdr-outset" => "outset",
        "bdr-trippl" => "tripple",
        "bdr-thsm__" => "thick-thin-small",
        "bdr-htsm__" => "thin-thick-small",
        "bdr-hthsm_" => "thin-thick-thin-small",
        "bdr-thm___" => "thick-thin-medium",
        "bdr-htm___" => "thin-thick-medium",
        "bdr-hthm__" => "thin-thick-thin-medium",
        "bdr-thl___" => "thick-thin-large",
        "bdr-hthl__" => "thin-thick-thin-large",
        "bdr-wavy__" => "wavy",
        "bdr-d-wav_" => "double-wavy",
        "bdr-strip_" => "striped",
        "bdr-embos_" => "emboss",
        "bdr-engra_" => "engrave",
        "bdr-frame_" => "frame",
        _ => return None,
    })
}

/// Port of `BorderParse.__determine_styles`'s fixed priority chain.
/// Preserved verbatim including its dead branches: `'engraved'` (the
/// real dict value is `'engrave'`) and `'tripple-border'` (the real
/// value is `'tripple'`) can never match, and `'thick-thin-small'` is
/// checked twice in a row. `'thin-thick-thin-large'` has no dedicated
/// branch at all. All four fall through to the trailing
/// `border_style_list[0]` fallback instead of their "intended" slot in
/// the priority order -- harmless when a border has only one style
/// keyword (the overwhelmingly common case), but a real priority-order
/// bug if it ever has several with one of those four among them.
fn determine_styles(
    border_type: &str,
    border_style_list: &[&'static str],
) -> BTreeMap<String, String> {
    let att = format!("{border_type}-style");
    let mut out = BTreeMap::new();
    let contains = |name: &str| border_style_list.contains(&name);
    let picked = if contains("shadowed-border") {
        Some("shadowed")
    } else if contains("engraved") {
        Some("engraved") // dead: real value is "engrave"
    } else if contains("emboss") {
        Some("emboss")
    } else if contains("striped") {
        Some("striped")
    } else if contains("thin-thick-thin-small") {
        Some("thin-thick-thin-small")
    } else if contains("thick-thin-large") {
        Some("thick-thin-large")
    } else if contains("thin-thick-thin-medium") {
        Some("thin-thick-thin-medium")
    } else if contains("thin-thick-medium") {
        Some("thin-thick-medium")
    } else if contains("thick-thin-medium") {
        Some("thick-thin-medium")
    } else if contains("thick-thin-small") {
        Some("thick-thin-small")
    } else if contains("thick-thin-small") {
        // duplicate branch, preserved verbatim
        Some("thick-thin-small")
    } else if contains("double-wavy") {
        Some("double-wavy")
    } else if contains("dot-dot-dash") {
        Some("dot-dot-dash")
    } else if contains("dot-dash") {
        Some("dot-dash")
    } else if contains("dotted-border") {
        Some("dotted")
    } else if contains("wavy") {
        Some("wavy")
    } else if contains("dash-small") {
        Some("dash-small")
    } else if contains("dashed") {
        Some("dashed")
    } else if contains("frame") {
        Some("frame")
    } else if contains("inset") {
        Some("inset")
    } else if contains("outset") {
        Some("outset")
    } else if contains("tripple-border") {
        Some("tripple") // dead: real value is "tripple"
    } else if contains("double-border") {
        Some("double")
    } else if contains("double-thickness-border") {
        Some("double-thickness")
    } else if contains("hairline") {
        Some("hairline")
    } else if contains("single") {
        Some("single")
    } else {
        border_style_list.first().copied()
    };
    if let Some(v) = picked {
        out.insert(att, v.to_string());
    }
    out
}

fn value_field(line: &str) -> &str {
    if line.len() > 20 {
        &line[20..]
    } else {
        ""
    }
}

/// Port of `BorderParse.parse_border`, private to this module (see the
/// module doc for why).
fn parse_border(line: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let key = if line.len() >= 16 { &line[6..16] } else { "" };
    let Some(border_type) = border_dict(key) else {
        eprintln!(
            "module is border_parse.py\nfunction is parse_border\ntoken does not have a dictionary value\ntoken is \"{line}\""
        );
        return out;
    };
    let att_line = value_field(line);
    let atts: Vec<&str> = att_line.split('|').collect();
    if atts.len() == 1 && atts[0].is_empty() {
        out.insert(border_type.to_string(), "none".to_string());
        return out;
    }
    let mut border_style_list: Vec<&'static str> = Vec::new();
    for att in atts {
        let (att_key, value) = match att.split_once(':') {
            Some((k, v)) => (k, v),
            None => (att, "true"),
        };
        if let Some(style_att) = border_style_dict(att_key) {
            border_style_list.push(style_att);
            // Note: the Python also writes this into a local
            // `border_style_dict` variable that's built and then never
            // read again -- omitted here as genuinely dead.
        } else {
            match border_dict(att_key) {
                Some(mapped) => {
                    out.insert(format!("{border_type}-{mapped}"), value.to_string());
                }
                None => {
                    eprintln!(
                        "module is border_parse_def.py\nfunction is parse_border\ntoken does not have an att value\nline is \"{line}\""
                    );
                    // Preserved quirk: Python's `att = f'{border_type}-{att}'`
                    // runs unconditionally even when the lookup above
                    // failed (`att` is `None` there), so the key
                    // becomes the literal `"{border_type}-None"`.
                    out.insert(format!("{border_type}-None"), value.to_string());
                }
            }
        }
    }
    out.extend(determine_styles(border_type, &border_style_list));
    out
}

// ---------------------------------------------------------------------
// ParagraphDef
// ---------------------------------------------------------------------

/// Port of `__token_dict`: converts a resolved token's fixed-width
/// label into the readable attribute name used in output.
fn token_dict(label: &str) -> Option<&'static str> {
    Some(match label {
        "par-end___" => "para",
        "par-def___" => "paragraph-definition",
        "keep-w-nex" => "keep-with-next",
        "widow-cntl" => "widow-control",
        "adjust-rgt" => "adjust-right",
        "language__" => "language",
        "right-inde" => "right-indent",
        "fir-ln-ind" => "first-line-indent",
        "left-inden" => "left-indent",
        "space-befo" => "space-before",
        "space-afte" => "space-after",
        "line-space" => "line-spacing",
        "default-ta" => "default-tab",
        "align_____" => "align",
        "widow-cntr" => "widow-control",
        "style-shet" => "stylesheet",
        "based-on__" => "based-on-style",
        "next-style" => "next-style",
        "char-style" => "character-style",
        "para-style" => "name",
        "picture___" => "pict",
        "obj-class_" => "obj_class",
        "mac-pic___" => "mac-pict",
        "section___" => "section-new",
        "sect-defin" => "section-reset",
        "sect-note_" => "endnotes-in-section",
        "list-text_" => "list-text",
        "list______" => "list",
        "list-lev-d" => "list-level-definition",
        "list-cardi" => "list-cardinal-numbering",
        "list-decim" => "list-decimal-numbering",
        "list-up-al" => "list-uppercase-alphabetic-numbering",
        "list-up-ro" => "list-uppercae-roman-numbering",
        "list-ord__" => "list-ordinal-numbering",
        "list-ordte" => "list-ordinal-text-numbering",
        "list-bulli" => "list-bullet",
        "list-simpi" => "list-simple",
        "list-conti" => "list-continue",
        "list-hang_" => "list-hang",
        "list-id___" => "list-id",
        "list-start" => "list-start",
        "nest-level" => "nest-level",
        "list-level" => "list-level",
        "footnote__" => "footnote",
        "type______" => "type",
        "toc_______" => "anchor-toc",
        "book-mk-st" => "bookmark-start",
        "book-mk-en" => "bookmark-end",
        "index-mark" => "anchor-index",
        "place_____" => "place",
        "field_____" => "field",
        "field-inst" => "field-instruction",
        "field-rslt" => "field-result",
        "datafield_" => "data-field",
        "font-table" => "font-table",
        "colr-table" => "color-table",
        "lovr-table" => "list-override-table",
        "listtable_" => "list-table",
        "revi-table" => "revision-table",
        "hidden____" => "hidden",
        "italics___" => "italics",
        "bold______" => "bold",
        "strike-thr" => "strike-through",
        "shadow____" => "shadow",
        "outline___" => "outline",
        "small-caps" => "small-caps",
        "caps______" => "caps",
        "dbl-strike" => "double-strike-through",
        "emboss____" => "emboss",
        "engrave___" => "engrave",
        "subscript_" => "subscript",
        "superscrip" => "superscipt",
        "font-style" => "font-style",
        "font-color" => "font-color",
        "font-size_" => "font-size",
        "font-up___" => "superscript",
        "font-down_" => "subscript",
        "red_______" => "red",
        "blue______" => "blue",
        "green_____" => "green",
        "row-def___" => "row-definition",
        "cell______" => "cell",
        "row_______" => "row",
        "in-table__" => "in-table",
        "columns___" => "columns",
        "row-pos-le" => "row-position-left",
        "cell-posit" => "cell-position",
        "underlined" => "underlined",
        "bor-t-r-hi" => "border-table-row-horizontal-inside",
        "bor-t-r-vi" => "border-table-row-vertical-inside",
        "bor-t-r-to" => "border-table-row-top",
        "bor-t-r-le" => "border-table-row-left",
        "bor-t-r-bo" => "border-table-row-bottom",
        "bor-t-r-ri" => "border-table-row-right",
        "bor-cel-bo" => "border-cell-bottom",
        "bor-cel-to" => "border-cell-top",
        "bor-cel-le" => "border-cell-left",
        "bor-cel-ri" => "border-cell-right",
        "bor-par-to" => "border-paragraph-top",
        "bor-par-le" => "border-paragraph-left",
        "bor-par-ri" => "border-paragraph-right",
        "bor-par-bo" => "border-paragraph-box",
        "bor-for-ev" => "border-for-every-paragraph",
        "bor-outsid" => "border-outisde",
        "bor-none__" => "border",
        "bdr-single" => "single",
        "bdr-doubtb" => "double-thickness-border",
        "bdr-shadow" => "shadowed-border",
        "bdr-double" => "double-border",
        "bdr-dotted" => "dotted-border",
        "bdr-dashed" => "dashed",
        "bdr-hair__" => "hairline",
        "bdr-inset_" => "inset",
        "bdr-das-sm" => "dash-small",
        "bdr-dot-sm" => "dot-dash",
        "bdr-dot-do" => "dot-dot-dash",
        "bdr-outset" => "outset",
        "bdr-trippl" => "tripple",
        "bdr-thsm__" => "thick-thin-small",
        "bdr-htsm__" => "thin-thick-small",
        "bdr-hthsm_" => "thin-thick-thin-small",
        "bdr-thm__" => "thick-thin-medium",
        "bdr-htm__" => "thin-thick-medium",
        "bdr-hthm_" => "thin-thick-thin-medium",
        "bdr-thl__" => "thick-thin-large",
        "bdr-hthl_" => "think-thick-think-large",
        "bdr-wavy_" => "wavy",
        "bdr-d-wav" => "double-wavy",
        "bdr-strip" => "striped",
        "bdr-embos" => "emboss",
        "bdr-engra" => "engrave",
        "bdr-frame" => "frame",
        "bdr-li-wid" => "line-width",
        _ => return None,
    })
}

/// Port of `__tab_type_dict`.
fn tab_type_dict(token: &str) -> Option<&'static str> {
    Some(match token {
        "cw<pf<tab-center" => "center",
        "cw<pf<tab-right_" => "right",
        "cw<pf<tab-dec___" => "decimal",
        "cw<pf<leader-dot" => "leader-dot",
        "cw<pf<leader-hyp" => "leader-hyphen",
        "cw<pf<leader-und" => "leader-underline",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Before1stParaDef,
    CollectTokens,
    AfterParaDef,
    InParagraphs,
    AfterParaEnd,
}

struct State {
    stage: Stage,
    att_val_dict: BTreeMap<String, String>,
    tab_type: String,
    text_string: String,
    style_num_strings: Vec<String>,
    body_style_strings: Vec<String>,
    default_font: String,
}

const START_MARKER: &str = "mi<mk<pard-start\n";
const START2_MARKER: &str = "mi<mk<pardstart_\n";
const END2_MARKER: &str = "mi<mk<pardend___\n";
const END_MARKER: &str = "mi<mk<pard-end__\n";

impl State {
    fn new(default_font: &str) -> Self {
        let mut s = State {
            stage: Stage::Before1stParaDef,
            att_val_dict: BTreeMap::new(),
            tab_type: "left".to_string(),
            text_string: String::new(),
            style_num_strings: Vec::new(),
            body_style_strings: Vec::new(),
            default_font: default_font.to_string(),
        };
        s.reset_dict();
        s
    }

    /// Port of `__reset_dict`.
    fn reset_dict(&mut self) {
        self.att_val_dict.clear();
        self.att_val_dict
            .insert("name".to_string(), "Normal".to_string());
        self.att_val_dict
            .insert("font-style".to_string(), self.default_font.clone());
        self.tab_type = "left".to_string();
        for key in [
            "tabs-left",
            "tabs-right",
            "tabs-center",
            "tabs-decimal",
            "tabs-bar",
            "tabs",
        ] {
            self.att_val_dict.insert(key.to_string(), String::new());
        }
    }

    /// Port of `__get_num_of_style`.
    fn get_num_of_style(&mut self) {
        const IGNORE: &[&str] = &["style-num", "nest-level", "in-table"];
        let mut sig = String::new();
        for (k, v) in &self.att_val_dict {
            if !IGNORE.contains(&k.as_str()) {
                sig.push_str(&format!("{k}:{v}"));
            }
        }
        let (num, new_style) = match self.style_num_strings.iter().position(|s| s == &sig) {
            Some(idx) => (idx + 1, false),
            None => {
                self.style_num_strings.push(sig);
                (self.style_num_strings.len(), true)
            }
        };
        self.att_val_dict
            .insert("style-num".to_string(), format!("s{num:04}"));
        if new_style {
            self.write_body_styles();
        }
    }

    /// Port of `__write_body_styles`.
    fn write_body_styles(&mut self) {
        const TABS_LIST: &[&str] = &[
            "tabs-left",
            "tabs-right",
            "tabs-decimal",
            "tabs-center",
            "tabs-bar",
            "tabs",
        ];
        let mut s = String::from("mi<tg<empty-att_<paragraph-style-in-body");
        s.push_str(&format!("<name>{}", self.att_val_dict["name"]));
        s.push_str(&format!("<style-number>{}", self.att_val_dict["style-num"]));
        if !self.att_val_dict["tabs"].is_empty() {
            s.push_str(&format!("<tabs>{}", self.att_val_dict["tabs"]));
        }
        let mut exclude: Vec<&str> = vec!["name", "style-num", "in-table"];
        exclude.extend_from_slice(TABS_LIST);
        for (k, v) in &self.att_val_dict {
            if !exclude.contains(&k.as_str()) {
                s.push_str(&format!("<{k}>{v}"));
            }
        }
        s.push('\n');
        self.body_style_strings.push(s);
    }

    /// Port of `__write_para_def_beg`.
    fn write_para_def_beg(&mut self, out: &mut String) {
        self.get_num_of_style();
        if self
            .att_val_dict
            .get("in-table")
            .is_some_and(|v| !v.is_empty())
        {
            out.push_str("mi<mk<in-table__\n");
        } else {
            out.push_str("mi<mk<not-in-tbl\n");
        }
        if let Some(li) = self.att_val_dict.get("left-indent") {
            if !li.is_empty() {
                out.push_str(&format!("mi<mk<left_inden<{li}\n"));
            }
        }
        let is_list = self
            .att_val_dict
            .get("list-id")
            .filter(|v| !v.is_empty())
            .cloned();
        if let Some(id) = &is_list {
            out.push_str(&format!("mi<mk<list-id___<{id}\n"));
        } else {
            out.push_str("mi<mk<no-list___\n");
        }
        out.push_str(&format!("mi<mk<style-name<{}\n", self.att_val_dict["name"]));
        out.push_str(START_MARKER);
        out.push_str("mi<tg<open-att__<paragraph-definition");
        out.push_str(&format!("<name>{}", self.att_val_dict["name"]));
        out.push_str(&format!("<style-number>{}", self.att_val_dict["style-num"]));
        const TABS_LIST: &[&str] = &[
            "tabs-left",
            "tabs-right",
            "tabs-decimal",
            "tabs-center",
            "tabs-bar",
            "tabs",
        ];
        if !self.att_val_dict["tabs"].is_empty() {
            out.push_str(&format!("<tabs>{}", self.att_val_dict["tabs"]));
        }
        let mut exclude: Vec<&str> = vec!["name", "style-num", "in-table"];
        exclude.extend_from_slice(TABS_LIST);
        for (k, v) in &self.att_val_dict {
            if !exclude.contains(&k.as_str()) {
                out.push_str(&format!("<{k}>{v}"));
            }
        }
        out.push('\n');
        out.push_str(START2_MARKER);
        if let Some(face) = self.att_val_dict.get("font-style") {
            out.push_str(&format!("mi<mk<font______<{face}\n"));
        }
        if let Some(caps) = self.att_val_dict.get("caps") {
            out.push_str(&format!("mi<mk<caps______<{caps}\n"));
        }
    }

    /// Port of `__write_para_def_end_func`.
    fn write_para_def_end(&mut self, out: &mut String) {
        out.push_str(END2_MARKER);
        out.push_str("mi<tg<close_____<paragraph-definition\n");
        out.push_str(END_MARKER);
        out.push_str(&self.text_string);
        self.text_string.clear();
        if self.att_val_dict.contains_key("font-style") {
            out.push_str("mi<mk<font-end__\n");
        }
        if self.att_val_dict.contains_key("caps") {
            out.push_str("mi<mk<caps-end__\n");
        }
    }
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 {
        &line[..16]
    } else {
        line
    }
}

/// Result of [`make_paragraph_def`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagraphDefOutput {
    pub content: String,
    /// Port of the Python's return value: threaded into
    /// [`super::body_styles::insert_info`].
    pub body_style_strings: Vec<String>,
}

/// Port of `ParagraphDef.make_paragraph_def`, operating directly on
/// intermediate-format content (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file.
pub fn make_paragraph_def(
    content: &str,
    default_font: &str,
    run_level: u32,
) -> Result<ParagraphDefOutput> {
    let mut st = State::new(default_font);
    let mut out = String::new();

    for line in content.lines() {
        let info = token_info(line);
        match st.stage {
            Stage::Before1stParaDef => {
                if info == "cw<pf<par-def___" {
                    st.stage = Stage::CollectTokens;
                    st.reset_dict();
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::CollectTokens => {
                if info == "mi<mk<para-start" {
                    st.write_para_def_beg(&mut out);
                    out.push_str(line);
                    out.push('\n');
                    st.stage = Stage::InParagraphs;
                } else if info == "cw<pf<par-def___" {
                    st.stage = Stage::CollectTokens;
                    st.reset_dict();
                } else if info == "cw<tb<cell______" || info == "cw<tb<row_______" {
                    out.push_str("mi<mk<in-table__\n");
                    out.push_str(line);
                    out.push('\n');
                    st.stage = Stage::AfterParaDef;
                } else if line.len() < 2 || &line[..2] != "cw" {
                    out.push_str(line);
                    out.push('\n');
                    st.stage = Stage::AfterParaDef;
                } else if line.len() >= 5 && &line[..5] == "cw<bd" {
                    let parsed = parse_border(line);
                    st.att_val_dict.extend(parsed);
                } else if let Some(handler) = tabs_dict_handler(info) {
                    dispatch_tab(handler, &mut st, line, info, run_level)?;
                } else if line.len() >= 16 {
                    if let Some(token) = token_dict(&line[6..16]) {
                        st.att_val_dict
                            .insert(token.to_string(), value_field(line).to_string());
                    }
                }
            }
            Stage::AfterParaDef => {
                if info == "cw<pf<par-def___" {
                    st.stage = Stage::CollectTokens;
                    st.reset_dict();
                } else if info == "mi<mk<para-start" {
                    st.write_para_def_beg(&mut out);
                    out.push_str(line);
                    out.push('\n');
                    st.stage = Stage::InParagraphs;
                } else if info == "cw<tb<cell______" || info == "cw<tb<row_______" {
                    out.push_str("mi<mk<in-table__\n");
                    out.push_str(line);
                    out.push('\n');
                    st.stage = Stage::AfterParaDef;
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::InParagraphs => {
                if info == "mi<mk<para-end__" {
                    st.stage = Stage::AfterParaEnd;
                    out.push_str(line);
                    out.push('\n');
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Stage::AfterParaEnd => {
                st.text_string.push_str(line);
                st.text_string.push('\n');
                match info {
                    "mi<mk<para-start" | "mi<mk<para-end__" => {
                        st.stage = Stage::InParagraphs;
                        out.push_str(&st.text_string);
                        st.text_string.clear();
                    }
                    "cw<pf<par-def___" => {
                        st.write_para_def_end(&mut out);
                        st.stage = Stage::CollectTokens;
                        st.reset_dict();
                    }
                    "mi<mk<body-close" | "mi<mk<par-in-fld" | "cw<tb<cell______"
                    | "cw<tb<row-def___" | "cw<tb<row_______" | "mi<mk<sect-close"
                    | "mi<mk<sect-start" | "mi<mk<header-beg" | "mi<mk<header-end"
                    | "mi<mk<head___clo" | "mi<mk<fldbk-end_" | "mi<mk<lst-txbeg_" => {
                        st.write_para_def_end(&mut out);
                        st.stage = Stage::AfterParaDef;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(ParagraphDefOutput {
        content: out,
        body_style_strings: st.body_style_strings,
    })
}

#[derive(Debug, Clone, Copy)]
enum TabHandler {
    Stop,
    Type,
    Leader,
    Bar,
}

fn tabs_dict_handler(info: &str) -> Option<TabHandler> {
    Some(match info {
        "cw<pf<tab-stop__" => TabHandler::Stop,
        "cw<pf<tab-center" | "cw<pf<tab-right_" | "cw<pf<tab-dec___" => TabHandler::Type,
        "cw<pf<leader-dot" | "cw<pf<leader-hyp" | "cw<pf<leader-und" => TabHandler::Leader,
        "cw<pf<tab-bar-st" => TabHandler::Bar,
        _ => return None,
    })
}

fn dispatch_tab(
    handler: TabHandler,
    st: &mut State,
    line: &str,
    info: &str,
    run_level: u32,
) -> Result<()> {
    match handler {
        TabHandler::Stop => {
            let entry = st.att_val_dict.entry("tabs".to_string()).or_default();
            entry.push_str(&format!("{}:", st.tab_type));
            entry.push_str(&format!("{};", value_field(line)));
            st.tab_type = "left".to_string();
        }
        TabHandler::Type => match tab_type_dict(info) {
            Some(t) => st.tab_type = t.to_string(),
            None => {
                if run_level > 3 {
                    return Err(ParagraphDefError::NoTabEntry(info.to_string()));
                }
            }
        },
        TabHandler::Leader => match tab_type_dict(info) {
            Some(leader) => {
                let entry = st.att_val_dict.entry("tabs".to_string()).or_default();
                entry.push_str(&format!("{leader}^"));
            }
            None => {
                if run_level > 3 {
                    return Err(ParagraphDefError::NoTabEntry(info.to_string()));
                }
            }
        },
        TabHandler::Bar => {
            let entry = st.att_val_dict.entry("tabs".to_string()).or_default();
            entry.push_str(&format!("bar:{};", value_field(line)));
            st.tab_type = "left".to_string();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    #[test]
    fn preamble_text_before_any_pard_passes_through() {
        let content = lines(&["tx<nu<__________<preamble text"]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        assert_eq!(out.content, content);
        assert!(out.body_style_strings.is_empty());
    }

    #[test]
    fn simple_paragraph_gets_wrapped_in_definition_tags() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<para-start",
            "tx<nu<__________<hello",
            "mi<mk<para-end__",
        ]);
        let out = make_paragraph_def(&content, "Times New Roman", 1).unwrap();
        assert!(out
            .content
            .contains("mi<tg<open-att__<paragraph-definition"));
        assert!(out.content.contains("<align>left"));
        assert!(out.content.contains("<name>Normal"));
        assert!(out.content.contains("mi<mk<pard-start"));
        assert_eq!(out.body_style_strings.len(), 1);
        assert!(out.body_style_strings[0].contains("paragraph-style-in-body"));
        assert!(out.body_style_strings[0].contains("<name>Normal"));
        assert!(out.body_style_strings[0].contains("<style-number>s0001"));
    }

    #[test]
    fn identical_paragraph_defs_share_one_style_number_and_one_body_style_string() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<para-start",
            "tx<nu<__________<one",
            "mi<mk<para-end__",
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<para-start",
            "tx<nu<__________<two",
            "mi<mk<para-end__",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        // only one distinct style signature -> one body style string,
        // and both paragraphs reference style-number s0001.
        assert_eq!(out.body_style_strings.len(), 1);
        assert_eq!(out.content.matches("<style-number>s0001").count(), 2);
    }

    #[test]
    fn differing_paragraph_defs_get_distinct_style_numbers() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<left",
            "mi<mk<para-start",
            "tx<nu<__________<one",
            "mi<mk<para-end__",
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<cent",
            "mi<mk<para-start",
            "tx<nu<__________<two",
            "mi<mk<para-end__",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        assert_eq!(out.body_style_strings.len(), 2);
        assert!(out.content.contains("<style-number>s0001"));
        assert!(out.content.contains("<style-number>s0002"));
    }

    #[test]
    fn tab_stop_and_leader_are_collected_into_tabs_attribute() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<pf<leader-dot<nu<true",
            "cw<pf<tab-stop__<nu<720",
            "mi<mk<para-start",
            "mi<mk<para-end__",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        assert!(out.content.contains("<tabs>leader-dot^left:720;"));
    }

    #[test]
    fn border_line_is_parsed_into_attributes() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<bd<bor-par-bo<nu<bdr-single",
            "mi<mk<para-start",
            "mi<mk<para-end__",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        // "bor-par-bo" maps to "border-paragraph-bottom" (not "-box" --
        // that's "bor-par-bx", a different key -- verified against
        // border_parse.py's own dict).
        assert!(out.content.contains("border-paragraph-bottom-style>single"));
    }

    #[test]
    fn unrecognized_tab_type_degrades_silently_below_run_level_four() {
        // tab_type_dict is fully covered by the tabs_dict routing table
        // (only tab-center/right_/dec___ route to TabHandler::Type, and
        // all three exist in tab_type_dict), so this exercises the
        // degrade path only reachable by direct unit testing of
        // dispatch_tab with a token that routes to Type but isn't in
        // tab_type_dict -- not reachable via the public token stream,
        // included for completeness of dispatch_tab's own contract.
        let mut st = State::new("Times");
        let result = dispatch_tab(
            TabHandler::Type,
            &mut st,
            "cw<pf<tab-right_<nu<1",
            "cw<pf<bogus_type",
            1,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn unrecognized_tab_type_raises_above_run_level_three() {
        let mut st = State::new("Times");
        let err = dispatch_tab(
            TabHandler::Type,
            &mut st,
            "cw<pf<tab-right_<nu<1",
            "cw<pf<bogus_type",
            4,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ParagraphDefError::NoTabEntry("cw<pf<bogus_type".to_string())
        );
    }

    #[test]
    fn stray_non_cw_line_in_collect_tokens_ends_paragraph_def_collection() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "mi<mk<body-close",
            "tx<nu<__________<orphan",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        // the stray non-cw line passes through and moves us to
        // after_para_def, where the next line (also non-matching) just
        // passes through too.
        assert!(out.content.contains("mi<mk<body-close"));
        assert!(out.content.contains("orphan"));
    }

    #[test]
    fn cell_token_during_collection_marks_in_table_and_moves_on() {
        let content = lines(&["cw<pf<par-def___<nu<true", "cw<tb<cell______<nu<true"]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        assert!(out.content.contains("mi<mk<in-table__"));
        assert!(out.content.contains("cw<tb<cell______<nu<true"));
    }

    #[test]
    fn font_style_and_caps_emit_start_and_end_markers() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "cw<ci<caps______<nu<true",
            "mi<mk<para-start",
            "mi<mk<para-end__",
            "mi<mk<body-close",
        ]);
        let out = make_paragraph_def(&content, "Courier", 1).unwrap();
        assert!(out.content.contains("mi<mk<font______<Courier"));
        assert!(out.content.contains("mi<mk<caps______<true"));
        assert!(out.content.contains("mi<mk<font-end__"));
        assert!(out.content.contains("mi<mk<caps-end__"));
    }

    #[test]
    fn new_paragraph_def_immediately_after_paragraph_end_closes_and_reopens() {
        let content = lines(&[
            "cw<pf<par-def___<nu<true",
            "mi<mk<para-start",
            "tx<nu<__________<first",
            "mi<mk<para-end__",
            "cw<pf<par-def___<nu<true",
            "cw<pf<align_____<nu<cent",
            "mi<mk<para-start",
            "tx<nu<__________<second",
            "mi<mk<para-end__",
            "mi<mk<body-close",
        ]);
        let out = make_paragraph_def(&content, "Times", 1).unwrap();
        assert_eq!(
            out.content
                .matches("mi<tg<close_____<paragraph-definition")
                .count(),
            2
        );
        assert_eq!(
            out.content
                .matches("mi<tg<open-att__<paragraph-definition")
                .count(),
            2
        );
        assert!(out.content.contains("first"));
        assert!(out.content.contains("second"));
    }
}
