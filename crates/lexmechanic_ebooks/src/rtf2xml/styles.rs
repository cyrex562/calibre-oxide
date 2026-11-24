//! Port of `old_src/src/calibre/ebooks/rtf2xml/styles.py` (`Styles`).
//!
//! Parses the RTF `\stylesheet` group into a per-style attribute table:
//! while inside the stylesheet, replaces the marker/control-word stream
//! with `mi<tg<...` tag lines describing each named paragraph/character
//! style's attributes (based-on, next-style, tabs, borders, etc); after
//! the stylesheet, rewrites every body `cw<ss<para-style<nu<{num}>` /
//! `cw<ss<char-style<nu<{num}>` line to reference the style's *name*
//! instead of its numeric index (falling back to a `not-defined`
//! marker for a style number the stylesheet never defined).
//!
//! Checkpoint `styles_info` in `ParseRtf.py`; runs immediately after
//! `colors.py`'s pass and immediately before `info.py`'s pass (both out
//! of scope here).
//!
//! Operates directly on the intermediate-format text (see
//! [`super::process_tokens`]'s module docs for the line shapes) rather
//! than reopening a file, matching [`super::check_brackets`]'s
//! convention.
//!
//! # `BorderParse` is inlined here, not shared
//!
//! The Python `Styles` delegates border-line parsing (`cw<bd<...`
//! lines) to a shared `border_parse.BorderParse` helper also used by
//! `paragraph_def.py` (see [`super::mod`]'s own docs on how
//! `border_parse` gets inlined per-consumer in this crate rather than
//! becoming its own `pub` module). Since this file may only touch
//! `styles.rs`, [`parse_border`] below is a private, from-scratch port
//! of `BorderParse.parse_border` scoped to this module -- it is not
//! shared with (and may end up duplicated by) whichever pass ports
//! `paragraph_def.py`.
//!
//! # Preserved upstream quirks (see inline docs at each site for detail)
//!
//! - [`StylesError::NoStyleWith`]: `__fix_based_on` builds an unused
//!   first diagnostic message that is immediately overwritten by a
//!   second before raising -- only the second ever surfaces.
//! - [`StylesError::TabsParEntryMissing`]: `__tab_stop_func` /
//!   `__tab_leader_func` / `__tab_bar_func` all hardcode the `'par'`
//!   style bucket for their `tabs` bookkeeping regardless of the
//!   current style's actual type, which can silently cross-contaminate
//!   a paragraph style's `tabs` entry with a character style's tab
//!   data, or -- if no paragraph style shares the number -- raise an
//!   uncaught `KeyError` in the Python (ported here as a normal `Err`,
//!   not a panic, per this crate's convention of never introducing a
//!   panic standing in for an upstream crash).
//! - `__tab_stop_func`'s `if self.__leader_found: ... else: ...` is a
//!   dead conditional: both branches are byte-for-byte identical.
//! - `__tab_leader_func`'s success path prepends a leading `:` to the
//!   appended leader string; its `except KeyError` fallback path does
//!   not -- an inconsistent format between the two, preserved exactly.
//! - `__tab_bar_func` resets `self.__tab_type` but -- unlike
//!   `__tab_stop_func` -- never resets `self.__leader_found`.
//! - `__para_style_in_body_func`'s "not defined" fallback tag is
//!   `cw<ss<{prefix}_style<nu<not-defined` (underscore) while the
//!   found-name case is `cw<ss<{prefix}-style<nu<{value}` (hyphen) --
//!   an inconsistent tag name preserved exactly (see
//!   [`para_style_in_body_func`]'s test coverage).
//! - [`parse_border`]: the local `border_style_dict` the Python builds
//!   is computed but never used/returned (only `border_dict` is) --
//!   not reproduced here since it has no observable effect. When a
//!   border sub-attribute's key isn't recognized by either lookup
//!   dict, the Python still proceeds to build `f'{border_type}-{att}'`
//!   with `att` bound to `None`, producing the literal string
//!   `"{border_type}-None"` as a dict key -- preserved exactly (see
//!   `parse_border_unrecognized_sub_attribute_uses_literal_none`).
//! - `__determine_styles`' border-style-name priority chain has a
//!   fully duplicated `'thick-thin-small'` branch, and two branches
//!   (`'engraved'`, `'tripple-border'`) that can never match because
//!   `BorderParse`'s own style-name dict only ever produces `'engrave'`
//!   and `'tripple'` (verified by inspection of that dict's literal
//!   values) -- all reproduced verbatim in [`DETERMINE_STYLES_PRIORITY`].
//! - `Styles.__token_dict`'s `# border => bd` section (`'bor-t-r-hi'`
//!   etc) is unreachable dead code: any line with the `cw<bd` prefix is
//!   always intercepted earlier, by `__in_individual_style_func`'s own
//!   explicit `line[0:5] == 'cw<bd'` branch, before the generic
//!   `__token_dict` lookup this section belongs to is ever consulted.
//!   Its neighboring `# border type => bt` section is reachable in
//!   principle (via a standalone `cw<bt<...` line that
//!   `combine_borders.py`, out of scope here, didn't merge into its
//!   `cw<bd<...` line first) but roughly half of its entries are
//!   themselves typo'd one character short of the fixed 10-char label
//!   width every real label uses (e.g. `'bdr-thm__'`, 9 characters, can
//!   never equal a real 10-char label slice) -- both preserved verbatim
//!   in [`token_dict`] with this explanation rather than "corrected".

use indexmap::IndexMap;
use thiserror::Error;

/// Errors [`convert_styles`] can return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StylesError {
    /// Port of the `run_level > 3` gated raise in
    /// `__in_individual_style_func`'s generic `cw` branch: an
    /// unrecognized paragraph/character-formatting control word (not
    /// in `__ignore_list`) with no entry in `__token_dict`. Below the
    /// threshold, the Python silently drops the token instead (no
    /// dictionary entry created, no error).
    #[error("no value for key {0}\n")]
    NoValueForKey(String),

    /// Port of the `run_level > 3` gated raise, identical at both call
    /// sites, in `__tab_type_func` and `__tab_leader_func`: a tab
    /// token whose `token_info` has no entry in `__tab_type_dict`.
    /// Should be unreachable in practice, since both are only ever
    /// dispatched for `token_info` values already known to be in that
    /// dict's key set (see [`tabs_action`]/[`tab_type_dict`]).
    #[error("no entry for {0}\n")]
    NoEntryFor(String),

    /// Port of the `run_level > 4` gated raise in `__fix_based_on`; see
    /// this module's doc comment for the discarded-first-message quirk
    /// this preserves.
    #[error("There is no style with {0}\n")]
    NoStyleWith(String),

    /// Port of a genuine *uncaught* Python `KeyError`; see this
    /// module's doc comment for the hardcoded-`'par'`-bucket quirk
    /// this models. Not `run_level`-gated (the Python's crash isn't
    /// either).
    #[error(
        "tab info for style {0} has no matching paragraph-style dictionary entry to \
         (mis)write into -- upstream KeyError equivalent"
    )]
    TabsParEntryMissing(String),
}

pub type Result<T> = std::result::Result<T, StylesError>;

/// Port of `line[start:end]` (Python slicing, tolerant of
/// out-of-range indices) for the fixed-width ASCII prefixes this
/// format's fields always occupy. `end = None` means "to the end of
/// the string" (the Python idiom `line[start:]`, or `line[start:-1]`
/// once the trailing `\n` `str::lines()` already stripped is accounted
/// for -- see [`super::check_brackets`]'s own doc comment on the same
/// adjustment).
fn py_slice(s: &str, start: usize, end: Option<usize>) -> &str {
    let len = s.len();
    let start = start.min(len);
    let end = end.map(|e| e.min(len)).unwrap_or(len);
    if start >= end {
        ""
    } else {
        &s[start..end]
    }
}

fn token_info(line: &str) -> &str {
    py_slice(line, 0, Some(16))
}

/// The `value` field of a `cw<{pre}<{label}<{subtype}<{value}` line
/// (see [`super::process_tokens`]'s module docs): `pre` (2) + `label`
/// (10) + `subtype` (2), each followed by a `<` delimiter, puts
/// `value` at a fixed byte offset of 20 -- port of the Python's
/// `line[20:-1]` (adjusted for `str::lines()` already having stripped
/// the trailing `\n`).
fn value_field(line: &str) -> &str {
    py_slice(line, 20, None)
}

// ---------------------------------------------------------------------
// Style-type / state machinery
// ---------------------------------------------------------------------

/// Port of `self.__type_of_style`. Modeled as an enum (rather than the
/// Python's bare string, which is compared against `'par'`/`'char'`
/// throughout) because it is provably always one of these two values:
/// initialized to `'par'` and only ever reassigned by `__para_style_func`
/// (`'par'`) or `__char_style_func` (`'char'`). This makes
/// `__add_dict_entry`'s `elif self.__run_level > 3: raise ...` catch-all
/// for "neither 'par' nor 'char'" -- itself already dead code, and, on
/// the `run_level <= 3` non-raising path, actually an `UnboundLocalError`
/// crash since `type_dict` would never get assigned -- structurally
/// unrepresentable here rather than reproduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StyleType {
    Par,
    Char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeforeStylesTable,
    InStylesTable,
    InIndividualStyle,
    AfterStylesTable,
}

/// The token-info-keyed subset of the Python's `self.__state_dict`
/// (which also holds the four `State` names as keys -- dead weight
/// here, since `token_info` derived from real line content never
/// equals a literal state name string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecialAction {
    StylesBeg,
    StylesEnd,
    StyleiBeg,
    StyleiEnd,
    ParaStyle,
    CharStyle,
}

fn special_action(info: &str) -> Option<SpecialAction> {
    match info {
        "mi<mk<styles-beg" => Some(SpecialAction::StylesBeg),
        "mi<mk<styles-end" => Some(SpecialAction::StylesEnd),
        "mi<mk<stylei-beg" => Some(SpecialAction::StyleiBeg),
        "mi<mk<stylei-end" => Some(SpecialAction::StyleiEnd),
        "cw<ss<para-style" => Some(SpecialAction::ParaStyle),
        "cw<ss<char-style" => Some(SpecialAction::CharStyle),
        _ => None,
    }
}

/// Port of `self.__tabs_dict`'s key set (`self.__tabs_list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabAction {
    Stop,
    Type,
    Leader,
    Bar,
}

fn tabs_action(info: &str) -> Option<TabAction> {
    match info {
        "cw<pf<tab-stop__" => Some(TabAction::Stop),
        "cw<pf<tab-center" | "cw<pf<tab-right_" | "cw<pf<tab-dec___" => Some(TabAction::Type),
        "cw<pf<leader-dot" | "cw<pf<leader-hyp" | "cw<pf<leader-und" => Some(TabAction::Leader),
        "cw<pf<tab-bar-st" => Some(TabAction::Bar),
        _ => None,
    }
}

/// Port of `self.__tab_type_dict`.
fn tab_type_dict(info: &str) -> Option<&'static str> {
    match info {
        "cw<pf<tab-center" => Some("center"),
        "cw<pf<tab-right_" => Some("right"),
        "cw<pf<tab-dec___" => Some("decimal"),
        "cw<pf<leader-dot" => Some("leader-dot"),
        "cw<pf<leader-hyp" => Some("leader-hyphen"),
        "cw<pf<leader-und" => Some("leader-underline"),
        _ => None,
    }
}

/// Port of `self.__ignore_list`.
const IGNORE_LIST: &[&str] = &["list-tebef"];

/// Port of `self.__token_dict`: maps the 10-char `label` field of a
/// generic `cw<pre<label<subtype<value` line (extracted as
/// `line[6:16]`) to a readable attribute name for the style-attribute
/// dictionary. See this module's doc comment for why the `# border =>
/// bd` entries are unconditionally dead and the `# border type => bt`
/// entries are only partly reachable.
fn token_dict(info: &str) -> Option<&'static str> {
    match info {
        // paragraph formatting => pf
        "par-end___" => Some("para"),
        "par-def___" => Some("paragraph-definition"),
        "keep-w-nex" => Some("keep-with-next"),
        "widow-cntl" => Some("widow-control"),
        "adjust-rgt" => Some("adjust-right"),
        "language__" => Some("language"),
        "right-inde" => Some("right-indent"),
        "fir-ln-ind" => Some("first-line-indent"),
        "left-inden" => Some("left-indent"),
        "space-befo" => Some("space-before"),
        "space-afte" => Some("space-after"),
        "line-space" => Some("line-spacing"),
        "default-ta" => Some("default-tab"),
        "align_____" => Some("align"),
        "widow-cntr" => Some("widow-control"),
        // page formatting mixed in! (Just in older RTF?)
        "margin-lef" => Some("left-indent"),
        "margin-rig" => Some("right-indent"),
        "margin-bot" => Some("space-after"),
        "margin-top" => Some("space-before"),
        // stylesheet => ss
        "style-shet" => Some("stylesheet"),
        "based-on__" => Some("based-on-style"),
        "next-style" => Some("next-style"),
        "char-style" => Some("character-style"),
        "para-style" => Some("paragraph-style"),
        // graphics => gr
        "picture___" => Some("pict"),
        "obj-class_" => Some("obj_class"),
        "mac-pic___" => Some("mac-pict"),
        // section => sc
        "section___" => Some("section-new"),
        "sect-defin" => Some("section-reset"),
        "sect-note_" => Some("endnotes-in-section"),
        // list => ls
        "list-text_" => Some("list-text"),
        "list______" => Some("list"),
        "list-lev-d" => Some("list-level-definition"),
        "list-cardi" => Some("list-cardinal-numbering"),
        "list-decim" => Some("list-decimal-numbering"),
        "list-up-al" => Some("list-uppercase-alphabetic-numbering"),
        "list-up-ro" => Some("list-uppercae-roman-numbering"),
        "list-ord__" => Some("list-ordinal-numbering"),
        "list-ordte" => Some("list-ordinal-text-numbering"),
        "list-bulli" => Some("list-bullet"),
        "list-simpi" => Some("list-simple"),
        "list-conti" => Some("list-continue"),
        "list-hang_" => Some("list-hang"),
        "list-id___" => Some("list-id"),
        "list-start" => Some("list-start"),
        "nest-level" => Some("nest-level"),
        "list-level" => Some("list-level"),
        // notes => nt
        "footnote__" => Some("footnote"),
        "type______" => Some("type"),
        // anchor => an
        "toc_______" => Some("anchor-toc"),
        "book-mk-st" => Some("bookmark-start"),
        "book-mk-en" => Some("bookmark-end"),
        "index-mark" => Some("anchor-index"),
        "place_____" => Some("place"),
        // field => fd
        "field_____" => Some("field"),
        "field-inst" => Some("field-instruction"),
        "field-rslt" => Some("field-result"),
        "datafield_" => Some("data-field"),
        // info-tables => it
        "font-table" => Some("font-table"),
        "colr-table" => Some("color-table"),
        "lovr-table" => Some("list-override-table"),
        "listtable_" => Some("list-table"),
        "revi-table" => Some("revision-table"),
        // character info => ci
        "hidden____" => Some("hidden"),
        "italics___" => Some("italics"),
        "bold______" => Some("bold"),
        "strike-thr" => Some("strike-through"),
        "shadow____" => Some("shadow"),
        "outline___" => Some("outline"),
        "small-caps" => Some("small-caps"),
        "dbl-strike" => Some("double-strike-through"),
        "emboss____" => Some("emboss"),
        "engrave___" => Some("engrave"),
        "subscript_" => Some("subscript"),
        "superscrip" => Some("superscript"),
        "plain_____" => Some("plain"),
        "font-style" => Some("font-style"),
        "font-color" => Some("font-color"),
        "font-size_" => Some("font-size"),
        "font-up___" => Some("superscript"),
        "font-down_" => Some("subscript"),
        "red_______" => Some("red"),
        "blue______" => Some("blue"),
        "green_____" => Some("green"),
        "caps______" => Some("caps"),
        // table => tb
        "row-def___" => Some("row-definition"),
        "cell______" => Some("cell"),
        "row_______" => Some("row"),
        "in-table__" => Some("in-table"),
        "columns___" => Some("columns"),
        "row-pos-le" => Some("row-position-left"),
        "cell-posit" => Some("cell-position"),
        // underline
        "underlined" => Some("underlined"),
        // border => bd -- DEAD, see module doc comment: any `cw<bd`
        // line is always intercepted earlier.
        "bor-t-r-hi" => Some("border-table-row-horizontal-inside"),
        "bor-t-r-vi" => Some("border-table-row-vertical-inside"),
        "bor-t-r-to" => Some("border-table-row-top"),
        "bor-t-r-le" => Some("border-table-row-left"),
        "bor-t-r-bo" => Some("border-table-row-bottom"),
        "bor-t-r-ri" => Some("border-table-row-right"),
        "bor-cel-bo" => Some("border-cell-bottom"),
        "bor-cel-to" => Some("border-cell-top"),
        "bor-cel-le" => Some("border-cell-left"),
        "bor-cel-ri" => Some("border-cell-right"),
        "bor-par-to" => Some("border-paragraph-top"),
        "bor-par-le" => Some("border-paragraph-left"),
        "bor-par-ri" => Some("border-paragraph-right"),
        "bor-par-bo" => Some("border-paragraph-box"),
        "bor-for-ev" => Some("border-for-every-paragraph"),
        "bor-outsid" => Some("border-outisde"),
        "bor-none__" => Some("border"),
        // border type => bt -- only reachable via a standalone
        // `cw<bt<...` line; entries from `bdr-thm__` onward are
        // additionally typo'd one char short of the real 10-char label
        // width and so can never match (kept verbatim, see module doc).
        "bdr-single" => Some("single"),
        "bdr-doubtb" => Some("double-thickness-border"),
        "bdr-shadow" => Some("shadowed-border"),
        "bdr-double" => Some("double-border"),
        "bdr-dotted" => Some("dotted-border"),
        "bdr-dashed" => Some("dashed"),
        "bdr-hair__" => Some("hairline"),
        "bdr-inset_" => Some("inset"),
        "bdr-das-sm" => Some("dash-small"),
        "bdr-dot-sm" => Some("dot-dash"),
        "bdr-dot-do" => Some("dot-dot-dash"),
        "bdr-outset" => Some("outset"),
        "bdr-trippl" => Some("tripple"),
        "bdr-thsm__" => Some("thick-thin-small"),
        "bdr-htsm__" => Some("thin-thick-small"),
        "bdr-hthsm_" => Some("thin-thick-thin-small"),
        "bdr-thm__" => Some("thick-thin-medium"),
        "bdr-htm__" => Some("thin-thick-medium"),
        "bdr-hthm_" => Some("thin-thick-thin-medium"),
        "bdr-thl__" => Some("thick-thin-large"),
        "bdr-hthl_" => Some("think-thick-think-large"),
        "bdr-wavy_" => Some("wavy"),
        "bdr-d-wav" => Some("double-wavy"),
        "bdr-strip" => Some("striped"),
        "bdr-embos" => Some("emboss"),
        "bdr-engra" => Some("engrave"),
        "bdr-frame" => Some("frame"),
        "bdr-li-wid" => Some("line-width"),
        // tabs
        "tab-center" => Some("center"),
        "tab-right_" => Some("right"),
        "tab-dec___" => Some("decimal"),
        "leader-dot" => Some("leader-dot"),
        "leader-hyp" => Some("leader-hyphen"),
        "leader-und" => Some("leader-underline"),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Border-line parsing (private port of `border_parse.BorderParse`)
// ---------------------------------------------------------------------

/// Port of `BorderParse.__border_dict`: border-attribute (and border
/// category) labels to readable names.
fn border_dict(key: &str) -> Option<&'static str> {
    match key {
        "bor-t-r-hi" => Some("border-table-row-horizontal-inside"),
        "bor-t-r-vi" => Some("border-table-row-vertical-inside"),
        "bor-t-r-to" => Some("border-table-row-top"),
        "bor-t-r-le" => Some("border-table-row-left"),
        "bor-t-r-bo" => Some("border-table-row-bottom"),
        "bor-t-r-ri" => Some("border-table-row-right"),
        "bor-cel-bo" => Some("border-cell-bottom"),
        "bor-cel-to" => Some("border-cell-top"),
        "bor-cel-le" => Some("border-cell-left"),
        "bor-cel-ri" => Some("border-cell-right"),
        "bor-par-bo" => Some("border-paragraph-bottom"),
        "bor-par-to" => Some("border-paragraph-top"),
        "bor-par-le" => Some("border-paragraph-left"),
        "bor-par-ri" => Some("border-paragraph-right"),
        "bor-par-bx" => Some("border-paragraph-box"),
        "bor-for-ev" => Some("border-for-every-paragraph"),
        "bor-outsid" => Some("border-outside"),
        "bor-none__" => Some("border"),
        // border type => bt
        "bdr-li-wid" => Some("line-width"),
        "bdr-sp-wid" => Some("padding"),
        "bdr-color_" => Some("color"),
        _ => None,
    }
}

/// Port of `BorderParse.__border_style_dict`.
fn border_style_dict(key: &str) -> Option<&'static str> {
    match key {
        "bdr-single" => Some("single"),
        "bdr-doubtb" => Some("double-thickness-border"),
        "bdr-shadow" => Some("shadowed-border"),
        "bdr-double" => Some("double-border"),
        "bdr-dotted" => Some("dotted-border"),
        "bdr-dashed" => Some("dashed"),
        "bdr-hair__" => Some("hairline"),
        "bdr-inset_" => Some("inset"),
        "bdr-das-sm" => Some("dash-small"),
        "bdr-dot-sm" => Some("dot-dash"),
        "bdr-dot-do" => Some("dot-dot-dash"),
        "bdr-outset" => Some("outset"),
        "bdr-trippl" => Some("tripple"),
        "bdr-thsm__" => Some("thick-thin-small"),
        "bdr-htsm__" => Some("thin-thick-small"),
        "bdr-hthsm_" => Some("thin-thick-thin-small"),
        "bdr-thm___" => Some("thick-thin-medium"),
        "bdr-htm___" => Some("thin-thick-medium"),
        "bdr-hthm__" => Some("thin-thick-thin-medium"),
        "bdr-thl___" => Some("thick-thin-large"),
        "bdr-hthl__" => Some("thin-thick-thin-large"),
        "bdr-wavy__" => Some("wavy"),
        "bdr-d-wav_" => Some("double-wavy"),
        "bdr-strip_" => Some("striped"),
        "bdr-embos_" => Some("emboss"),
        "bdr-engra_" => Some("engrave"),
        "bdr-frame_" => Some("frame"),
        _ => None,
    }
}

/// Port of `BorderParse.__determine_styles`'s if/elif priority chain,
/// in exact source order. See this module's doc comment: the
/// duplicated `"thick-thin-small"` entry and the `"engraved"` /
/// `"tripple-border"` entries (whose real [`border_style_dict`] values
/// are `"engrave"` / `"tripple"`, never these) are all dead, and kept
/// verbatim rather than pruned.
const DETERMINE_STYLES_PRIORITY: &[(&str, &str)] = &[
    ("shadowed-border", "shadowed"),
    ("engraved", "engraved"), // dead: real value is "engrave"
    ("emboss", "emboss"),
    ("striped", "striped"),
    ("thin-thick-thin-small", "thin-thick-thin-small"),
    ("thick-thin-large", "thick-thin-large"),
    ("thin-thick-thin-medium", "thin-thick-thin-medium"),
    ("thin-thick-medium", "thin-thick-medium"),
    ("thick-thin-medium", "thick-thin-medium"),
    ("thick-thin-small", "thick-thin-small"),
    ("thick-thin-small", "thick-thin-small"), // dead duplicate of the line above
    ("double-wavy", "double-wavy"),
    ("dot-dot-dash", "dot-dot-dash"),
    ("dot-dash", "dot-dash"),
    ("dotted-border", "dotted"),
    ("wavy", "wavy"),
    ("dash-small", "dash-small"),
    ("dashed", "dashed"),
    ("frame", "frame"),
    ("inset", "inset"),
    ("outset", "outset"),
    ("tripple-border", "tripple"), // dead: real value is "tripple"
    ("double-border", "double"),
    ("double-thickness-border", "double-thickness"),
    ("hairline", "hairline"),
    ("single", "single"),
];

/// Port of `BorderParse.__determine_styles`.
fn determine_styles(border_type: &str, border_style_list: &[String]) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    let att = format!("{border_type}-style");
    for (check, value) in DETERMINE_STYLES_PRIORITY {
        if border_style_list.iter().any(|s| s == check) {
            out.insert(att, value.to_string());
            return out;
        }
    }
    if let Some(first) = border_style_list.first() {
        out.insert(att, first.clone());
    }
    out
}

/// Port of `BorderParse.parse_border`, operating on the already
/// newline-stripped `line`. Infallible (matches the Python, which
/// never raises here -- only writes diagnostics to stderr and returns
/// a partial or empty dict).
fn parse_border(line: &str) -> IndexMap<String, String> {
    let mut result: IndexMap<String, String> = IndexMap::new();
    let label = py_slice(line, 6, Some(16));
    let Some(border_type) = border_dict(label) else {
        eprintln!(
            "module is border_parse.py\nfunction is parse_border\ntoken does not have a dictionary value\ntoken is \"{line}\""
        );
        return result;
    };

    let att_line = value_field(line);
    let atts: Vec<&str> = att_line.split('|').collect();
    if atts.len() == 1 && atts[0].is_empty() {
        result.insert(border_type.to_string(), "none".to_string());
        return result;
    }

    // Port of `border_style_dict` (the *local* variable, not the
    // module-level `border_style_dict` lookup function above): built
    // by the Python but never read after -- only `border_style_list`
    // (which feeds `__determine_styles`) and `border_dict` (this
    // function's `result`) matter for the return value, so it isn't
    // reproduced here.
    let mut border_style_list: Vec<String> = Vec::new();

    for raw_att in atts {
        let values: Vec<&str> = raw_att.split(':').collect();
        let (att, value) = if values.len() == 2 {
            (values[0], values[1].to_string())
        } else {
            (raw_att, "true".to_string())
        };
        if let Some(style_att) = border_style_dict(att) {
            // Port of `att = f'{border_type}-{att}'; border_style_dict[att]
            // = value` -- building the local `border_style_dict` entry
            // (the dead variable noted above) is skipped entirely since
            // nothing ever reads it; only `border_style_list` matters.
            border_style_list.push(style_att.to_string());
        } else {
            // Port of `att = self.__border_dict.get(att)`, then
            // unconditionally `att = f'{border_type}-{att}'` even when
            // that lookup returned `None` -- Python's f-string
            // stringifies `None` as the literal text `"None"`,
            // producing e.g. `"border-cell-right-None"` as the actual
            // dict key. Preserved verbatim; see this module's doc
            // comment.
            let resolved = border_dict(att);
            if resolved.is_none() {
                eprintln!(
                    "module is border_parse_def.py\nfunction is parse_border\ntoken does not have an att value\nline is \"{line}\""
                );
            }
            let att_name = resolved.unwrap_or("None");
            let key = format!("{border_type}-{att_name}");
            result.insert(key, value);
        }
    }

    let style_result = determine_styles(border_type, &border_style_list);
    result.extend(style_result);
    result
}

// ---------------------------------------------------------------------
// Main FSM
// ---------------------------------------------------------------------

/// Port of `Styles`'s per-call state, threaded explicitly instead of
/// as `self` fields.
struct Styles {
    state: State,
    par: IndexMap<String, IndexMap<String, String>>,
    char_styles: IndexMap<String, IndexMap<String, String>>,
    styles_num: String,
    type_of_style: StyleType,
    text_string: String,
    tab_type: String,
    leader_found: bool,
    output: String,
}

impl Styles {
    fn new() -> Self {
        Styles {
            state: State::BeforeStylesTable,
            par: IndexMap::new(),
            char_styles: IndexMap::new(),
            styles_num: "0".to_string(),
            type_of_style: StyleType::Par,
            text_string: String::new(),
            tab_type: "left".to_string(),
            leader_found: false,
            output: String::new(),
        }
    }

    fn bucket(&self, ty: StyleType) -> &IndexMap<String, IndexMap<String, String>> {
        match ty {
            StyleType::Par => &self.par,
            StyleType::Char => &self.char_styles,
        }
    }

    fn bucket_mut(&mut self, ty: StyleType) -> &mut IndexMap<String, IndexMap<String, String>> {
        match ty {
            StyleType::Par => &mut self.par,
            StyleType::Char => &mut self.char_styles,
        }
    }

    fn write_line(&mut self, line: &str) {
        self.output.push_str(line);
        self.output.push('\n');
    }

    /// Port of `__enter_dict_entry` (+ `__add_dict_entry`).
    fn enter_dict_entry(&mut self, att: &str, value: &str) {
        let ty = self.type_of_style;
        let num = self.styles_num.clone();
        let bucket = self.bucket_mut(ty);
        if let Some(style_map) = bucket.get_mut(&num) {
            style_map.insert(att.to_string(), value.to_string());
        } else {
            let mut m = IndexMap::new();
            m.insert(att.to_string(), value.to_string());
            bucket.insert(num, m);
        }
    }

    fn para_style_func(&mut self, line: &str) {
        self.type_of_style = StyleType::Par;
        self.styles_num = value_field(line).to_string();
    }

    fn char_style_func(&mut self, line: &str) {
        self.type_of_style = StyleType::Char;
        self.styles_num = value_field(line).to_string();
    }

    fn found_end_ind_style_func(&mut self) {
        let mut name = self.text_string.clone();
        name.pop(); // drop trailing ';' -- port of `text_string[:-1]`
        let name = name.trim().to_string();
        self.enter_dict_entry("name", &name);
        self.text_string.clear();
    }

    /// Port of `__fix_based_on`. `run_level`-gated at `> 4` (not the
    /// `> 3` most other gates in this module use).
    fn fix_based_on(&mut self, run_level: u32) -> Result<()> {
        for ty in [StyleType::Par, StyleType::Char] {
            let keys: Vec<String> = self.bucket(ty).keys().cloned().collect();
            for key in keys {
                for style_attr in ["next-style", "based-on-style"] {
                    let value = self
                        .bucket(ty)
                        .get(&key)
                        .and_then(|m| m.get(style_attr))
                        .cloned();
                    let Some(value) = value else { continue };
                    let temp = self.bucket(ty).get(&value).cloned();
                    match temp {
                        Some(temp_map) if !temp_map.is_empty() => {
                            if let Some(changed) = temp_map.get("name") {
                                if !changed.is_empty() {
                                    self.bucket_mut(ty)
                                        .get_mut(&key)
                                        .expect("key came from this bucket's own key list")
                                        .insert(style_attr.to_string(), changed.clone());
                                }
                            }
                        }
                        _ => {
                            // Port of `if value in {0, '0'}: pass` --
                            // `0` (the int) is dead (values here are
                            // always strings), so only `"0"` matters.
                            if value != "0" && run_level > 4 {
                                return Err(StylesError::NoStyleWith(value));
                            }
                            // Unconditional (unless the raise above
                            // fired): the Python's `del` sits outside
                            // the `if value in {0,'0'} / elif
                            // run_level > 4` chain, at the same
                            // indentation, so it runs regardless of
                            // which (if either) of those matched.
                            self.bucket_mut(ty)
                                .get_mut(&key)
                                .expect("key came from this bucket's own key list")
                                .shift_remove(style_attr);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Port of `__print_style_table`.
    fn print_style_table(&mut self) {
        let groups: [(StyleType, &str); 2] = [
            (StyleType::Par, "paragraph"),
            (StyleType::Char, "character"),
        ];
        for (ty, prefix) in groups {
            self.output
                .push_str(&format!("mi<tg<open______<{prefix}-styles\n"));
            let bucket = match ty {
                StyleType::Par => &self.par,
                StyleType::Char => &self.char_styles,
            };
            for (num, attrs) in bucket.iter() {
                self.output.push_str(&format!(
                    "mi<tg<empty-att_<{prefix}-style-in-table<num>{num}"
                ));
                for (att, val) in attrs.iter() {
                    self.output.push_str(&format!("<{att}>{val}"));
                }
                self.output.push('\n');
            }
            self.output
                .push_str(&format!("mi<tg<close_____<{prefix}-styles\n"));
        }
    }

    /// Port of the shared `state_dict` dispatch used by
    /// `__before_styles_func`, `__in_styles_func`, and
    /// `__in_individual_style_func` for their token-info-keyed special
    /// markers.
    fn dispatch_special(
        &mut self,
        action: SpecialAction,
        line: &str,
        run_level: u32,
    ) -> Result<()> {
        match action {
            SpecialAction::StylesBeg => self.state = State::InStylesTable,
            SpecialAction::StylesEnd => {
                self.state = State::AfterStylesTable;
                self.fix_based_on(run_level)?;
                self.print_style_table();
            }
            SpecialAction::StyleiBeg => self.state = State::InIndividualStyle,
            SpecialAction::StyleiEnd => self.found_end_ind_style_func(),
            SpecialAction::ParaStyle => self.para_style_func(line),
            SpecialAction::CharStyle => self.char_style_func(line),
        }
        Ok(())
    }

    /// Port of `__before_styles_func`.
    fn before_styles_func(&mut self, line: &str, info: &str, run_level: u32) -> Result<()> {
        match special_action(info) {
            Some(action) => self.dispatch_special(action, line, run_level),
            None => {
                self.write_line(line);
                Ok(())
            }
        }
    }

    /// Port of `__in_styles_func`.
    fn in_styles_func(&mut self, line: &str, info: &str, run_level: u32) -> Result<()> {
        match special_action(info) {
            Some(action) => self.dispatch_special(action, line, run_level),
            None => {
                self.write_line(line);
                Ok(())
            }
        }
    }

    /// Port of `__tab_stop_func`. See module doc comment: the
    /// `if self.__leader_found / else` split in the Python is a dead
    /// conditional (identical bodies), not reproduced as a branch here
    /// -- but `leader_found` is still reset at the end, matching the
    /// Python's actual observable behavior.
    fn tab_stop_func(&mut self, line: &str) -> Result<()> {
        let value = value_field(line).to_string();
        let addition = format!("{}:{value};", self.tab_type);
        self.append_par_tabs(&addition)?;
        self.tab_type = "left".to_string();
        self.leader_found = false;
        Ok(())
    }

    fn tab_type_func(&mut self, info: &str, run_level: u32) -> Result<()> {
        match tab_type_dict(info) {
            Some(t) => self.tab_type = t.to_string(),
            None => {
                if run_level > 3 {
                    return Err(StylesError::NoEntryFor(info.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Port of `__tab_leader_func`. See module doc comment for the
    /// asymmetric leading-`:` quirk between the success and
    /// KeyError-fallback paths, preserved exactly below.
    fn tab_leader_func(&mut self, info: &str, run_level: u32) -> Result<()> {
        self.leader_found = true;
        match tab_type_dict(info) {
            Some(leader) => {
                let leader = format!("{leader}^");
                let num = self.styles_num.clone();
                let has_tabs = self.par.get(&num).is_some_and(|m| m.contains_key("tabs"));
                if has_tabs {
                    self.par
                        .get_mut(&num)
                        .and_then(|m| m.get_mut("tabs"))
                        .expect("just checked has_tabs")
                        .push_str(&format!(":{leader};"));
                } else {
                    self.enter_dict_entry("tabs", "");
                    match self.par.get_mut(&num).and_then(|m| m.get_mut("tabs")) {
                        // NOTE: no leading ':' here, unlike the success
                        // path above -- verified upstream quirk.
                        Some(tabs) => tabs.push_str(&format!("{leader};")),
                        None => return Err(StylesError::TabsParEntryMissing(num)),
                    }
                }
                Ok(())
            }
            None => {
                if run_level > 3 {
                    Err(StylesError::NoEntryFor(info.to_string()))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Port of `__tab_bar_func`. Note: unlike [`Styles::tab_stop_func`],
    /// this does not reset `leader_found` -- verified upstream quirk.
    fn tab_bar_func(&mut self, line: &str) -> Result<()> {
        let value = value_field(line).to_string();
        let addition = format!("bar:{value};");
        self.append_par_tabs(&addition)?;
        self.tab_type = "left".to_string();
        Ok(())
    }

    /// Shared plumbing for the "append to the hardcoded `'par'`
    /// bucket's `tabs` entry, self-repairing via `__enter_dict_entry`
    /// on `KeyError`" shape common to `__tab_stop_func` and
    /// `__tab_bar_func` (both append a single already-assembled
    /// string, unlike `__tab_leader_func`'s asymmetric two-path
    /// formatting, kept inline there). See module doc comment for the
    /// hardcoded-`'par'` quirk this reproduces, including the
    /// possible `Err` when `type_of_style` is `Char`.
    fn append_par_tabs(&mut self, addition: &str) -> Result<()> {
        let num = self.styles_num.clone();
        let has_tabs = self.par.get(&num).is_some_and(|m| m.contains_key("tabs"));
        if has_tabs {
            self.par
                .get_mut(&num)
                .and_then(|m| m.get_mut("tabs"))
                .expect("just checked has_tabs")
                .push_str(addition);
            return Ok(());
        }
        self.enter_dict_entry("tabs", "");
        match self.par.get_mut(&num).and_then(|m| m.get_mut("tabs")) {
            Some(tabs) => {
                tabs.push_str(addition);
                Ok(())
            }
            None => Err(StylesError::TabsParEntryMissing(num)),
        }
    }

    /// Port of `__in_individual_style_func`.
    fn in_individual_style_func(&mut self, line: &str, info: &str, run_level: u32) -> Result<()> {
        if let Some(action) = special_action(info) {
            return self.dispatch_special(action, line, run_level);
        }
        if py_slice(line, 0, Some(5)) == "cw<bd" {
            for (k, v) in parse_border(line) {
                self.enter_dict_entry(&k, &v);
            }
            return Ok(());
        }
        if let Some(tab_action) = tabs_action(info) {
            return match tab_action {
                TabAction::Stop => self.tab_stop_func(line),
                TabAction::Type => self.tab_type_func(info, run_level),
                TabAction::Leader => self.tab_leader_func(info, run_level),
                TabAction::Bar => self.tab_bar_func(line),
            };
        }
        if py_slice(line, 0, Some(2)) == "cw" {
            let label = py_slice(line, 6, Some(16));
            match token_dict(label) {
                Some(att) => {
                    let value = value_field(line).to_string();
                    self.enter_dict_entry(att, &value);
                }
                None => {
                    if !IGNORE_LIST.contains(&label) && run_level > 3 {
                        return Err(StylesError::NoValueForKey(label.to_string()));
                    }
                }
            }
            return Ok(());
        }
        if py_slice(line, 0, Some(2)) == "tx" {
            self.text_string.push_str(py_slice(line, 17, None));
        }
        // NOTE: unlike `before_styles_func`/`in_styles_func`, there is
        // no pass-through fallback here -- any line inside an
        // individual style entry that matches none of the branches
        // above (e.g. a stray `ob`/`cb` bracket marker) is silently
        // dropped, matching the Python exactly (it has no trailing
        // `else: write_obj.write(line)`).
        Ok(())
    }

    /// Port of `__para_style_in_body_func`.
    fn para_style_in_body_func(&mut self, line: &str, ty: StyleType) {
        let prefix = match ty {
            StyleType::Par => "para",
            StyleType::Char => "char",
        };
        let num = value_field(line);
        let value = self.bucket(ty).get(num).and_then(|m| m.get("name"));
        match value {
            // `if value:` -- an empty (falsy) name string falls
            // through to the "not defined" branch too.
            Some(v) if !v.is_empty() => {
                self.output
                    .push_str(&format!("cw<ss<{prefix}-style<nu<{v}\n"));
            }
            _ => {
                // NOTE: underscore, not hyphen, in `{prefix}_style` --
                // verified upstream quirk, see module doc comment.
                self.output
                    .push_str(&format!("cw<ss<{prefix}_style<nu<not-defined\n"));
            }
        }
    }

    /// Port of `__after_styles_func`.
    fn after_styles_func(&mut self, line: &str, info: &str) {
        match info {
            "cw<ss<para-style" => self.para_style_in_body_func(line, StyleType::Par),
            "cw<ss<char-style" => self.para_style_in_body_func(line, StyleType::Char),
            _ => self.write_line(line),
        }
    }
}

/// Port of `Styles.convert_styles`, operating directly on the
/// intermediate-format text (see [`super::process_tokens`]'s module
/// docs) rather than reopening a file, and returning the transformed
/// content instead of mutating one in place.
///
/// The Python's driving `while` loop processes one extra, empty
/// `line_to_read == ''` iteration after the real input is exhausted
/// (an artifact of testing `readline()`'s return value for truthiness
/// *after* using it) before exiting; every state's handler is a
/// complete no-op for an empty line (`token_info` is `''`, matching no
/// special marker, no border/tab/`cw`/`tx` prefix, and the
/// pass-through branches write an empty string), so it has no
/// observable effect on the output and is not reproduced here.
pub fn convert_styles(content: &str, run_level: u32) -> Result<String> {
    let mut styles = Styles::new();
    for line in content.lines() {
        let info = token_info(line);
        match styles.state {
            State::BeforeStylesTable => styles.before_styles_func(line, info, run_level)?,
            State::InStylesTable => styles.in_styles_func(line, info, run_level)?,
            State::InIndividualStyle => styles.in_individual_style_func(line, info, run_level)?,
            State::AfterStylesTable => styles.after_styles_func(line, info),
        }
    }
    Ok(styles.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> String {
        v.join("\n") + "\n"
    }

    // ----------------------------------------------------------------
    // State-machine transitions / basic pass-through
    // ----------------------------------------------------------------

    #[test]
    fn lines_before_styles_table_pass_through_unchanged() {
        let content = lines(&["tx<nu<__________<hello", "cw<ci<bold______<nu<true"]);
        let out = convert_styles(&content, 1).unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn lines_between_styles_beg_and_first_individual_style_pass_through() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "tx<nu<__________<stray text",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // "stray text" is not consumed by any style, but the empty
        // (no styles defined) print-table output still appears.
        assert!(out.contains("tx<nu<__________<stray text"));
        assert!(out.contains("mi<tg<open______<paragraph-styles"));
        assert!(out.contains("mi<tg<close_____<paragraph-styles"));
        assert!(out.contains("mi<tg<open______<character-styles"));
        assert!(out.contains("mi<tg<close_____<character-styles"));
    }

    // ----------------------------------------------------------------
    // A full single-style walk: para-style, attributes, name, table
    // ----------------------------------------------------------------

    #[test]
    fn single_paragraph_style_produces_expected_table_row() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<widow-cntl<nu<true",
            "tx<nu<__________<Normal",
            "tx<nu<__________<;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert_eq!(
            out,
            "mi<tg<open______<paragraph-styles\n\
             mi<tg<empty-att_<paragraph-style-in-table<num>0<widow-control>true<name>Normal\n\
             mi<tg<close_____<paragraph-styles\n\
             mi<tg<open______<character-styles\n\
             mi<tg<close_____<character-styles\n"
        );
    }

    #[test]
    fn style_name_is_trimmed_of_surrounding_whitespace_and_trailing_semicolon() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<char-style<nu<3",
            "tx<nu<__________<  Emphasis ",
            "tx<nu<__________<;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("mi<tg<empty-att_<character-style-in-table<num>3<name>Emphasis\n"));
    }

    #[test]
    fn attribute_and_style_number_order_matches_insertion_order() {
        // Two styles, attributes added in a specific order, plus a
        // second style added after -- output must preserve both the
        // per-style attribute order and the style-number order.
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<5",
            "cw<pf<space-afte<nu<240",
            "cw<pf<space-befo<nu<0",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<stylei-beg<nu<0002",
            "cw<ss<para-style<nu<1",
            "tx<nu<__________<Heading;",
            "mi<mk<stylei-end<nu<0002",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        let expected = "mi<tg<open______<paragraph-styles\n\
             mi<tg<empty-att_<paragraph-style-in-table<num>5<space-after>240<space-before>0<name>Body\n\
             mi<tg<empty-att_<paragraph-style-in-table<num>1<name>Heading\n\
             mi<tg<close_____<paragraph-styles\n";
        assert!(out.starts_with(expected));
    }

    // ----------------------------------------------------------------
    // token_dict lookups: unknown key handling (ignore-list / run_level)
    // ----------------------------------------------------------------

    #[test]
    fn ignored_unknown_token_is_silently_dropped_regardless_of_run_level() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<list-tebef_<nu<true",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        // `list-tebef` is the ignore-listed label; run_level 10 would
        // otherwise raise for any other unrecognized label.
        let out = convert_styles(&content, 10).unwrap();
        assert!(out.contains("mi<tg<empty-att_<paragraph-style-in-table<num>0<name>"));
    }

    #[test]
    fn unrecognized_token_below_run_level_four_is_silently_dropped() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<zz<mystery___<nu<true",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 3).unwrap();
        assert!(!out.contains("mystery"));
    }

    #[test]
    fn unrecognized_token_above_run_level_three_raises() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<zz<mystery___<nu<true",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let err = convert_styles(&content, 4).unwrap_err();
        assert_eq!(err, StylesError::NoValueForKey("mystery___".to_string()));
    }

    // ----------------------------------------------------------------
    // fix_based_on
    // ----------------------------------------------------------------

    #[test]
    fn based_on_style_number_is_replaced_with_referenced_style_name() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<stylei-beg<nu<0002",
            "cw<ss<para-style<nu<1",
            "cw<ss<based-on__<nu<0",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0002",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("<based-on-style>Normal"));
    }

    #[test]
    fn based_on_reference_to_style_zero_that_is_undefined_is_silently_dropped() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<1",
            "cw<ss<based-on__<nu<0",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        // style 0 is never defined; run_level is high enough that a
        // *non*-zero missing reference would raise, but '0' is special
        // cased to never raise.
        let out = convert_styles(&content, 10).unwrap();
        assert!(!out.contains("based-on-style"));
    }

    #[test]
    fn based_on_reference_to_missing_nonzero_style_is_dropped_below_run_level_five() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<1",
            "cw<ss<next-style<nu<99",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 4).unwrap();
        assert!(!out.contains("next-style"));
    }

    #[test]
    fn based_on_reference_to_missing_nonzero_style_raises_above_run_level_four() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<1",
            "cw<ss<next-style<nu<99",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let err = convert_styles(&content, 5).unwrap_err();
        // Verified upstream quirk: the Python builds an unused first
        // diagnostic message, then overwrites it before raising -- so
        // only "There is no style with 99" ever actually surfaces.
        assert_eq!(err, StylesError::NoStyleWith("99".to_string()));
    }

    #[test]
    fn based_on_reference_to_style_with_empty_name_is_left_unchanged() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "tx<nu<__________<;", // name becomes "" after stripping ';'
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<stylei-beg<nu<0002",
            "cw<ss<para-style<nu<1",
            "cw<ss<based-on__<nu<0",
            "tx<nu<__________<Body;",
            "mi<mk<stylei-end<nu<0002",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // `changed_value` (the empty name) is falsy, so the numeric
        // reference "0" survives unchanged rather than being replaced
        // -- and, since `temp_dict` (style 0) DID exist, the delete
        // branch is not taken either.
        assert!(out.contains("<based-on-style>0"));
    }

    // ----------------------------------------------------------------
    // Body pass: number -> name substitution, "not defined" fallback
    // ----------------------------------------------------------------

    #[test]
    fn body_style_reference_is_rewritten_to_style_name() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
            "cw<ss<para-style<nu<0",
            "tx<nu<__________<body text",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("cw<ss<para-style<nu<Normal\n"));
    }

    #[test]
    fn body_reference_to_undefined_style_number_uses_underscored_not_defined_tag() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<styles-end<nu<0001",
            "cw<ss<char-style<nu<7",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // Verified upstream quirk: the fallback tag name uses an
        // underscore ("char_style"), not a hyphen ("char-style") like
        // the successful-lookup case.
        assert!(out.contains("cw<ss<char_style<nu<not-defined\n"));
        assert!(!out.contains("cw<ss<char-style<nu<not-defined"));
    }

    // ----------------------------------------------------------------
    // Tabs: normal accumulation + verified quirks
    // ----------------------------------------------------------------

    #[test]
    fn tab_stop_accumulates_type_and_position() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<tab-right_<nu<true",
            "cw<pf<tab-stop__<nu<720",
            "cw<pf<tab-stop__<nu<1440",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // first stop uses the "right" type set just before it; the
        // second reverts to the default "left" (tab_type is reset
        // after each stop).
        assert!(out.contains("<tabs>right:720;left:1440;"));
    }

    #[test]
    fn tab_leader_adds_caret_suffixed_leader_with_leading_colon_on_first_use() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<leader-dot<nu<true",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // Verified upstream quirk: the *first* leader use goes through
        // the `except KeyError` fallback path (no "tabs" entry exists
        // yet), which -- unlike the "already exists" success path --
        // omits the leading ':'.
        assert!(out.contains("<tabs>leader-dot^;"));
    }

    #[test]
    fn tab_leader_second_use_on_existing_tabs_entry_keeps_leading_colon() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<tab-bar-st<nu<100",
            "cw<pf<leader-dot<nu<true",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // "tabs" already exists (from the bar stop), so this leader
        // append goes through the success path and keeps its ':'.
        assert!(out.contains("<tabs>bar:100;:leader-dot^;"));
    }

    #[test]
    fn tab_bar_does_not_reset_leader_found_unlike_tab_stop() {
        // leader-dot sets leader_found=true; tab-bar-st does NOT reset
        // it (verified quirk); a following tab-stop then sees
        // leader_found=true -- but since the if/else branches of
        // tab_stop_func are identical, output is unaffected either way.
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<pf<leader-dot<nu<true",
            "cw<pf<tab-bar-st<nu<100",
            "cw<pf<tab-stop__<nu<720",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("<tabs>leader-dot^;bar:100;left:720;"));
    }

    #[test]
    fn tab_type_unknown_token_below_run_level_four_is_ignored() {
        // tab_type_dict/tabs_action are internally consistent (every
        // Type-dispatched token_info has an entry), so this can only
        // be exercised by calling the function directly.
        let mut st = Styles::new();
        st.state = State::InIndividualStyle;
        assert!(st.tab_type_func("cw<pf<tab-unknwn", 1).is_ok());
        assert_eq!(st.tab_type, "left"); // unchanged
    }

    #[test]
    fn tab_stop_on_character_style_with_no_matching_paragraph_style_errors() {
        // Verified upstream quirk (uncaught Python KeyError): tab
        // bookkeeping hardcodes the 'par' bucket regardless of
        // self.__type_of_style. A tab-stop inside a character style
        // with no paragraph style sharing its number has nowhere valid
        // to (mis)write into.
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<char-style<nu<9",
            "cw<pf<tab-stop__<nu<720",
            "tx<nu<__________<Emphasis;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let err = convert_styles(&content, 1).unwrap_err();
        assert_eq!(err, StylesError::TabsParEntryMissing("9".to_string()));
    }

    #[test]
    fn tab_stop_on_character_style_cross_contaminates_matching_paragraph_style() {
        // Same quirk as above, but this time a paragraph style with
        // the same number *does* already have a "tabs" entry -- so the
        // character style's tab data silently lands there instead of
        // erroring or landing in the character style's own entry.
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<9",
            "cw<pf<tab-stop__<nu<100",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<stylei-beg<nu<0002",
            "cw<ss<char-style<nu<9",
            "cw<pf<tab-stop__<nu<720",
            "tx<nu<__________<Emphasis;",
            "mi<mk<stylei-end<nu<0002",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // The character style's tab-stop (720) ends up appended to the
        // *paragraph* style's tabs entry, not the character style's.
        assert!(out.contains(
            "mi<tg<empty-att_<paragraph-style-in-table<num>9<tabs>left:100;left:720;<name>Normal\n"
        ));
        assert!(out.contains("mi<tg<empty-att_<character-style-in-table<num>9<name>Emphasis\n"));
    }

    // ----------------------------------------------------------------
    // Border-line handling
    // ----------------------------------------------------------------

    #[test]
    fn border_line_is_parsed_into_style_attributes() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<bd<bor-par-bx<nu<bdr-single:20|bdr-sp-wid:10",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("<border-paragraph-box-style>single"));
        assert!(out.contains("<border-paragraph-box-padding>10"));
    }

    #[test]
    fn border_line_with_no_value_sets_none_marker() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<bd<bor-cel-ri<nu<",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        assert!(out.contains("<border-cell-right>none"));
    }

    #[test]
    fn cw_bd_line_always_wins_over_generic_token_dict_lookup() {
        // Demonstrates why `token_dict`'s "# border => bd" entries
        // (e.g. "bor-cel-ri") are dead: this line matches the explicit
        // `cw<bd` prefix check first and is routed through
        // `parse_border`, never reaching the generic `cw` branch that
        // would consult `token_dict`.
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<bd<bor-cel-ri<nu<bdr-single",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let out = convert_styles(&content, 1).unwrap();
        // parse_border's own naming ("border-cell-right...") is used,
        // not token_dict's differently-cased dead entry for the same
        // label ("border-cell-right" happens to agree here, but the
        // routing -- via parse_border, not the generic cw handler at
        // all -- is what this test actually pins down).
        assert!(out.contains("<border-cell-right-style>single"));
    }

    #[test]
    fn parse_border_unrecognized_border_category_returns_empty() {
        let result = parse_border("cw<bd<zzzzzzzzzz<nu<true");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_border_unrecognized_sub_attribute_uses_literal_none() {
        // Verified upstream quirk: an unrecognized sub-attribute
        // resolves to Python's `None`, which the following f-string
        // stringifies as the literal text "None" in the output key.
        let result = parse_border("cw<bd<bor-cel-ri<nu<not-a-real-attr:5");
        assert_eq!(
            result.get("border-cell-right-None").map(String::as_str),
            Some("5")
        );
    }

    #[test]
    fn determine_styles_duplicate_and_typo_branches_never_fire() {
        // "engrave" (the real value BorderParse's style dict produces)
        // should win via its own (reachable) branch, not the dead
        // "engraved" typo branch above it in priority order.
        let out = determine_styles("border-cell-right", &["engrave".to_string()]);
        assert_eq!(
            out.get("border-cell-right-style").map(String::as_str),
            Some("engrave")
        );

        // "tripple" similarly must resolve via the fallback (first
        // element), since "tripple-border" never matches.
        let out = determine_styles("border-cell-right", &["tripple".to_string()]);
        assert_eq!(
            out.get("border-cell-right-style").map(String::as_str),
            Some("tripple")
        );
    }

    // ----------------------------------------------------------------
    // token_dict's dead "bt" typo'd entries (documented, not "fixed")
    // ----------------------------------------------------------------

    #[test]
    fn token_dict_bt_typo_entry_never_matches_a_real_ten_char_label() {
        // "bdr-thm__" is stored 9 characters wide; a real label field
        // extracted via `line[6:16]` is always exactly 10 characters
        // (for a line at least 16 bytes long), so this lookup always
        // misses even for what was clearly meant to be a
        // "thick-thin-medium" border-type token.
        assert_eq!(token_dict("bdr-thm___"), None);
        // The correctly-10-char-wide early entries in the same section
        // ARE reachable in principle.
        assert_eq!(token_dict("bdr-single"), Some("single"));
    }

    #[test]
    fn generic_cw_bt_token_with_typo_d_dict_entry_degrades_or_raises_like_any_unknown_key() {
        let content = lines(&[
            "mi<mk<styles-beg<nu<0001",
            "mi<mk<stylei-beg<nu<0001",
            "cw<ss<para-style<nu<0",
            "cw<bt<bdr-thm___<nu<true",
            "tx<nu<__________<Normal;",
            "mi<mk<stylei-end<nu<0001",
            "mi<mk<styles-end<nu<0001",
        ]);
        let err = convert_styles(&content, 4).unwrap_err();
        assert_eq!(err, StylesError::NoValueForKey("bdr-thm___".to_string()));
    }
}
