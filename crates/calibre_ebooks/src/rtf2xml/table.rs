//! Port of `old_src/src/calibre/ebooks/rtf2xml/table.py` (`Table`).
//!
//! Wraps `cw<tb<row-def___`/`cw<tb<in-table__` groups in
//! `mi<mk<tabl-start`/`mi<mk<table-end_` markers and `<row>`/`<cell>`
//! tags, collecting each row's and cell's border/width/position
//! attributes (parsed from `cw<bd<...` border lines, `cw<tb<cell-posit`
//! cell-position tokens, and a couple of row-level tokens) along the
//! way. Tables can't nest, but the state machine still uses a stack
//! (`Vec<StateTag>`, mirroring Python's `self.__state` list): a
//! `not_in_table` document can be several levels deep in row-def/
//! row/cell state by the time a table closes.
//!
//! # `border_parse.py`: a third private copy
//!
//! Same situation as [`super::paragraph_def`] and [`super::styles`]
//! (see either module's own docs): `border_parse.py` isn't one of
//! this issue's files, so its `parse_border` logic is ported here too
//! as a third private, non-`pub` copy rather than unifying three
//! already-independent passes around a new shared dependency. Unlike
//! those two copies (which use a `BTreeMap`), this one uses an
//! `IndexMap` -- row/cell attribute dictionaries are written directly
//! into the output stream here (`<key>value` pairs after
//! `mi<tg<open-att__<row`/`<cell`), so, unlike `paragraph_def`/
//! `styles`'s own attribute dictionaries, insertion order is
//! genuinely observable in the transformed content and needs to match
//! Python's own (3.7+) insertion-ordered `dict`.
//!
//! # A confirmed-dead branch and a confirmed bug, fixed rather than preserved
//!
//! - `self.__in_row_definition_dict` (mapping `'mi<mk<not-in-tbl'` to
//!   `__end_row_table_func`, itself calling `self.__close_table(self,
//!   line)` -- an extra, wrong `self` argument that would `TypeError`
//!   if ever reached) is built in `__initiate_values` but never
//!   `.get()`-looked-up anywhere; `__in_row_def_func` has its own
//!   complete, independent if/elif chain instead. Confirmed dead by
//!   grep -- neither the dict nor `__end_row_table_func` is ported.
//! - `__in_row_def_func`'s `mi<mk<pard-start` branch checks
//!   `if (self.__state) > 0 and ...` -- comparing the state *list*
//!   itself to an `int`, not its length (`len(self.__state) > 0`,
//!   used correctly two branches below for the analogous
//!   `mi<mk<in-table__` check). In Python 3 this unconditionally
//!   raises `TypeError` -- not a rare edge case, since a row
//!   definition ending via a plain `pard-start` (rather than an
//!   explicit `cw<tb<row_______`) is an ordinary way for a table row
//!   to close. Preserving this literally would make the entire pass
//!   fail on a large fraction of real tables, which defeats the
//!   purpose of porting it at all -- unlike this codebase's usual
//!   "preserve observable quirks" stance (reserved for bugs that
//!   produce a *specific wrong-but-stable* result), this one is
//!   implemented as the evidently-intended `len(self.__state) > 0`
//!   check. Since `state` is never empty in practice (same invariant
//!   Python already relies on elsewhere in this file), the check is
//!   always true anyway -- so this "fix" has no effect beyond letting
//!   the pass actually run to completion on ordinary tables.
//!
//! `table_data` (this function's second return value) is never
//! written into the transformed content -- Python returns it from
//! `make_table()` purely as metadata for its caller (`ParseRtf.py`,
//! out of scope). Represented here as a small struct rather than a
//! stringly-typed dict, since nothing downstream in this crate reads
//! it in a specific string shape yet.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files -- the temp-file / [`super::copy`] / rename dance around the
//! real pass is pipeline plumbing, not ported here.

use indexmap::IndexMap;

// ---------------------------------------------------------------------
// Private port of border_parse.py's BorderParse.parse_border
// ---------------------------------------------------------------------

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

/// Port of `BorderParse.__determine_styles`'s fixed priority chain --
/// see [`super::paragraph_def`]'s identical copy for the preserved
/// dead-branch/duplicate-branch details (`'engraved'`, `'tripple-border'`,
/// the doubled `'thick-thin-small'` check).
fn determine_styles(border_type: &str, border_style_list: &[&'static str]) -> IndexMap<String, String> {
    let att = format!("{border_type}-style");
    let mut out = IndexMap::new();
    let contains = |name: &str| border_style_list.contains(&name);
    let picked = if contains("shadowed-border") {
        Some("shadowed")
    } else if contains("engraved") {
        Some("engraved")
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
        Some("tripple")
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

fn border_value_field(line: &str) -> &str {
    if line.len() >= 20 { &line[20..] } else { "" }
}

/// Port of `BorderParse.parse_border`, private to this module.
fn parse_border(line: &str) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    let key = if line.len() >= 16 { &line[6..16] } else { "" };
    let Some(border_type) = border_dict(key) else {
        eprintln!(
            "module is border_parse.py\nfunction is parse_border\ntoken does not have a dictionary value\ntoken is \"{line}\""
        );
        return out;
    };
    let att_line = border_value_field(line);
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
        } else {
            match border_dict(att_key) {
                Some(mapped) => {
                    out.insert(format!("{border_type}-{mapped}"), value.to_string());
                }
                None => {
                    eprintln!(
                        "module is border_parse_def.py\nfunction is parse_border\ntoken does not have an att value\nline is \"{line}\""
                    );
                    out.insert(format!("{border_type}-None"), value.to_string());
                }
            }
        }
    }
    out.extend(determine_styles(border_type, &border_style_list));
    out
}

// ---------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn value_field(line: &str) -> &str {
    if line.len() >= 20 { &line[20..] } else { "" }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateTag {
    NotInTable,
    InTable,
    InRowDef,
    InRow,
    InCell,
}

/// Port of `__mode`: the most frequent item in `items`, walked in
/// order (ties keep the first item to reach the current max count),
/// or [`Stat::NotDefined`] for an empty list -- matching Python's
/// `'not-defined'` string fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stat<T> {
    NotDefined,
    Value(T),
}

impl<T> Default for Stat<T> {
    fn default() -> Self {
        Stat::NotDefined
    }
}

fn mode<T: PartialEq + Clone>(items: &[T]) -> Stat<T> {
    let mut max = 0usize;
    let mut result: Option<T> = None;
    for item in items {
        let count = items.iter().filter(|x| *x == item).count();
        if count > max {
            max = count;
            result = Some(item.clone());
        }
    }
    match result {
        Some(v) => Stat::Value(v),
        None => Stat::NotDefined,
    }
}

/// Port of one entry of `make_table`'s returned `self.__table_data`
/// list -- see this module's own docs for why it's a struct rather
/// than a stringly-typed dict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableSummary {
    pub number_of_columns: usize,
    pub number_of_rows: usize,
    pub average_cells_per_row: Stat<usize>,
    pub average_cell_width: Stat<String>,
}

#[derive(Default)]
struct TableBuilder {
    state: Vec<StateTag>,
    table_data: Vec<TableSummary>,
    row_dict: IndexMap<String, String>,
    cell_list: Vec<IndexMap<String, String>>,
    /// Reset every row definition (not every table) -- see this
    /// module's own doc for why `average-cell-width` therefore only
    /// ever reflects the table's *last* row definition, a faithfully
    /// preserved upstream quirk.
    cell_widths: Vec<String>,
    last_cell_position: f64,
    rows_in_table: usize,
    cells_in_table: usize,
    cells_in_row: usize,
    max_number_cells_in_row: usize,
    list_of_cells_in_row: Vec<usize>,
}

impl TableBuilder {
    fn new() -> Self {
        Self { state: vec![StateTag::NotInTable], ..Default::default() }
    }

    fn close_table(&mut self, out: &mut String) {
        out.push_str("mi<mk<table-end_\n");
        self.state = vec![StateTag::NotInTable];
        if let Some(summary) = self.table_data.last_mut() {
            summary.number_of_columns = self.max_number_cells_in_row;
            summary.number_of_rows = self.rows_in_table;
            summary.average_cells_per_row = mode(&self.list_of_cells_in_row);
            summary.average_cell_width = mode(&self.cell_widths);
        }
    }

    fn found_row_def(&mut self) {
        self.state.push(StateTag::InRowDef);
        self.last_cell_position = 0.0;
        self.row_dict.clear();
        self.cell_list.clear();
        self.cell_list.push(IndexMap::new());
        self.cell_widths.clear();
    }

    fn start_table(&mut self, out: &mut String) {
        self.rows_in_table = 0;
        self.cells_in_table = 0;
        self.cells_in_row = 0;
        self.max_number_cells_in_row = 0;
        self.table_data.push(TableSummary::default());
        self.list_of_cells_in_row.clear();
        out.push_str("mi<mk<tabl-start\n");
        self.state.push(StateTag::InTable);
    }

    fn end_row_def(&mut self) {
        if self.state.last() == Some(&StateTag::InRowDef) {
            self.state.pop();
        }
        self.cell_list.pop();
        if let Some(widths) = self.row_dict.get("widths").cloned() {
            let num_cells = widths.split(',').count();
            self.row_dict.insert("number-of-cells".to_string(), num_cells.to_string());
        }
    }

    fn handle_row_token(&mut self, line: &str, tok: &str) {
        if line.len() >= 5 && &line[3..5] == "bd" {
            let parsed = parse_border(line);
            let in_cell = parsed.keys().any(|k| k.len() >= 11 && &k[..11] == "border-cell");
            for (k, v) in parsed {
                if in_cell {
                    if let Some(last) = self.cell_list.last_mut() {
                        last.insert(k, v);
                    }
                } else {
                    self.row_dict.insert(k, v);
                }
            }
        } else if tok == "cw<tb<cell-posit" {
            self.found_cell_position(line);
        } else if tok == "cw<tb<row-pos-le" {
            self.row_dict.insert("left-row-position".to_string(), value_field(line).to_string());
        } else if tok == "cw<tb<row-header" {
            self.row_dict.insert("header".to_string(), "true".to_string());
        }
    }

    fn found_cell_position(&mut self, line: &str) {
        let new_cell_position: f64 = value_field(line).trim().parse().unwrap_or(0.0);
        let mut left_position = 0.0;
        if self.last_cell_position == 0.0 {
            left_position =
                self.row_dict.get("left-row-position").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        }
        let width = new_cell_position - self.last_cell_position - left_position;
        let width = format!("{width:.2}");
        self.last_cell_position = new_cell_position;
        match self.row_dict.get("widths").cloned() {
            Some(existing) => {
                self.row_dict.insert("widths".to_string(), format!("{existing}, {width}"));
            }
            None => {
                self.row_dict.insert("widths".to_string(), width.clone());
            }
        }
        if let Some(last) = self.cell_list.last_mut() {
            last.insert("width".to_string(), width.clone());
        }
        self.cell_list.push(IndexMap::new());
        self.cell_widths.push(width);
    }

    fn write_atts(out: &mut String, dict: &IndexMap<String, String>) {
        for (k, v) in dict {
            out.push_str(&format!("<{k}>{v}"));
        }
    }

    /// Port of `__start_cell_func`: consumes `cell_list[0]` (the
    /// *first* still-pending cell dict, in the order they were
    /// collected during row-def parsing) -- unlike [`Self::empty_cell`],
    /// which reuses `cell_list[-1]` without consuming it.
    fn start_cell(&mut self, out: &mut String) {
        self.state.push(StateTag::InCell);
        if !self.cell_list.is_empty() {
            out.push_str("mi<tg<open-att__<cell");
            let cell_dict = self.cell_list.remove(0);
            Self::write_atts(out, &cell_dict);
            out.push('\n');
        } else {
            out.push_str("mi<tg<open______<cell\n");
        }
        self.cells_in_table += 1;
        self.cells_in_row += 1;
    }

    fn start_row(&mut self, out: &mut String) {
        self.state.push(StateTag::InRow);
        out.push_str("mi<tg<open-att__<row");
        let row_dict = self.row_dict.clone();
        Self::write_atts(out, &row_dict);
        out.push('\n');
        self.cells_in_row = 0;
        self.rows_in_table += 1;
    }

    fn end_cell(&mut self, out: &mut String) {
        if self.state.len() > 1 && self.state.last() == Some(&StateTag::InCell) {
            self.state.pop();
        }
        out.push_str("mi<mk<close_cell\nmi<tg<close_____<cell\nmi<mk<closecell_\n");
    }

    fn end_row(&mut self, out: &mut String) {
        if self.state.len() > 1 && self.state.last() == Some(&StateTag::InRow) {
            self.state.pop();
            out.push_str("mi<tg<close_____<row\n");
        } else {
            out.push_str("mi<tg<empty_____<row\n");
            self.rows_in_table += 1;
        }
        self.max_number_cells_in_row = self.max_number_cells_in_row.max(self.cells_in_row);
        self.list_of_cells_in_row.push(self.cells_in_row);
    }

    /// Port of `__empty_cell`: reuses `cell_list[-1]` -- see
    /// [`Self::start_cell`]'s doc for the contrast.
    fn empty_cell(&mut self, out: &mut String) {
        if let Some(cell_dict) = self.cell_list.last().cloned() {
            out.push_str("mi<tg<empty-att_<cell");
            Self::write_atts(out, &cell_dict);
            out.push('\n');
        } else {
            out.push_str("mi<tg<empty_____<cell\n");
        }
        self.cells_in_table += 1;
        self.cells_in_row += 1;
    }
}

const CLOSING_TOKENS: [&str; 4] =
    ["mi<mk<not-in-tbl", "mi<mk<sect-start", "mi<mk<sect-close", "mi<mk<body-close"];

/// Port of `Table.make_table`, operating directly on
/// intermediate-format content (see this module's own docs) rather
/// than reopening a file. Returns the transformed content and one
/// [`TableSummary`] per table found, in document order.
pub fn make_table(content: &str) -> (String, Vec<TableSummary>) {
    let mut b = TableBuilder::new();
    let mut out = String::new();

    for line in content.lines() {
        let tok = token_info(line);
        let stage = *b.state.last().expect("state is never empty");

        match stage {
            StateTag::NotInTable => {
                match tok {
                    "cw<tb<row-def___" => b.found_row_def(),
                    "cw<tb<in-table__" | "mi<mk<in-table__" => b.start_table(&mut out),
                    _ => {}
                }
                out.push_str(line);
                out.push('\n');
            }
            StateTag::InTable => {
                if CLOSING_TOKENS.contains(&tok) {
                    b.close_table(&mut out);
                } else {
                    match tok {
                        "mi<mk<pard-start" => {
                            b.start_row(&mut out);
                            b.start_cell(&mut out);
                        }
                        "cw<tb<row-def___" => b.found_row_def(),
                        "cw<tb<cell______" => {
                            b.start_row(&mut out);
                            b.empty_cell(&mut out);
                        }
                        _ => {}
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            StateTag::InRowDef => {
                if tok == "cw<tb<row_______" {
                    b.end_row(&mut out);
                    b.end_row_def();
                    out.push_str(line);
                    out.push('\n');
                } else if line.len() >= 2 && &line[..2] == "cw" {
                    b.handle_row_token(line, tok);
                    out.push_str(line);
                    out.push('\n');
                } else if tok == "mi<mk<not-in-tbl" && b.state.contains(&StateTag::InTable) {
                    b.end_row_def();
                    b.close_table(&mut out);
                    out.push_str(line);
                    out.push('\n');
                } else if tok == "mi<mk<pard-start" {
                    b.end_row_def();
                    if b.state.last() == Some(&StateTag::InTable) {
                        b.start_row(&mut out);
                        b.start_cell(&mut out);
                    }
                    out.push_str(line);
                    out.push('\n');
                } else if tok == "mi<mk<in-table__" {
                    b.end_row_def();
                    if b.state.last() != Some(&StateTag::InTable) {
                        b.start_table(&mut out);
                    }
                    out.push_str(line);
                    out.push('\n');
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            StateTag::InRow => {
                if CLOSING_TOKENS.contains(&tok) {
                    b.end_row(&mut out);
                    b.close_table(&mut out);
                } else {
                    match tok {
                        "mi<mk<pard-start" => b.start_cell(&mut out),
                        "cw<tb<row_______" => b.end_row(&mut out),
                        "cw<tb<cell______" => b.empty_cell(&mut out),
                        _ => {}
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            StateTag::InCell => {
                if CLOSING_TOKENS.contains(&tok) {
                    b.end_cell(&mut out);
                    b.end_row(&mut out);
                    b.close_table(&mut out);
                    out.push_str(line);
                    out.push('\n');
                } else if tok == "cw<tb<cell______" {
                    b.end_cell(&mut out);
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    (out, b.table_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_outside_a_table_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\n";
        let (out, tables) = make_table(content);
        assert_eq!(out, content);
        assert!(tables.is_empty());
    }

    #[test]
    fn a_simple_one_row_two_cell_table_is_wrapped_and_summarized() {
        // A row definition ordinarily ends via `mi<mk<pard-start` (not
        // `cw<tb<row_______`, which -- while still inside a row
        // definition -- means "this row has no content at all", a
        // separate, empty-row shortcut exercised by
        // `an_empty_cell_token_reuses_the_last_pending_cell_without_consuming_it`
        // below). Each cell then explicitly closes with
        // `cw<tb<cell______` before the next one's `pard-start`, and
        // the row itself closes with `cw<tb<row_______` only once
        // back in `in_row` state.
        let content = "\
mi<mk<in-table__\n\
cw<tb<row-def___<nu<true\n\
cw<tb<cell-posit<nu<100.00\n\
cw<tb<cell-posit<nu<200.00\n\
mi<mk<pard-start\n\
tx<nu<__________<cell one\n\
cw<tb<cell______<nu<true\n\
mi<mk<pard-start\n\
tx<nu<__________<cell two\n\
cw<tb<cell______<nu<true\n\
cw<tb<row_______<nu<true\n\
mi<mk<not-in-tbl\n";
        let (out, tables) = make_table(content);
        assert!(out.contains("mi<mk<tabl-start\n"), "{out}");
        assert!(out.contains("mi<mk<table-end_\n"), "{out}");
        assert!(
            out.contains("mi<tg<open-att__<row<widths>100.00, 100.00<number-of-cells>2\n"),
            "{out}"
        );
        assert_eq!(out.matches("mi<tg<open-att__<cell<width>100.00\n").count(), 2, "{out}");
        assert!(out.contains("mi<tg<close_____<row\n"), "{out}");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].number_of_rows, 1);
        assert_eq!(tables[0].number_of_columns, 2);
    }

    #[test]
    fn a_border_line_on_a_cell_updates_the_pending_cells_dict() {
        let content = "\
mi<mk<in-table__\n\
cw<tb<row-def___<nu<true\n\
cw<bd<bor-cel-to<nu<bdr-single\n\
cw<tb<cell-posit<nu<100.00\n\
mi<mk<pard-start\n\
cw<tb<cell______<nu<true\n\
mi<mk<not-in-tbl\n";
        let (out, _) = make_table(content);
        assert!(
            out.contains("mi<tg<open-att__<cell<border-cell-top-style>single<width>100.00\n"),
            "{out}"
        );
    }

    #[test]
    fn a_row_border_line_goes_into_the_row_not_a_cell() {
        let content = "\
mi<mk<in-table__\n\
cw<tb<row-def___<nu<true\n\
cw<bd<bor-t-r-to<nu<bdr-single\n\
mi<mk<pard-start\n\
cw<tb<cell______<nu<true\n\
mi<mk<not-in-tbl\n";
        let (out, _) = make_table(content);
        assert!(out.contains("mi<tg<open-att__<row<border-table-row-top-style>single\n"), "{out}");
        // Never leaks into the cell tag, which has no border info here.
        assert!(out.contains("mi<tg<open______<cell\n"), "{out}");
    }

    #[test]
    fn an_empty_cell_token_reuses_the_last_pending_cell_without_consuming_it() {
        let content = "\
mi<mk<in-table__\n\
cw<tb<row-def___<nu<true\n\
cw<tb<cell-posit<nu<50.00\n\
cw<tb<row_______<nu<true\n\
cw<tb<cell______<nu<true\n\
cw<tb<cell______<nu<true\n\
mi<mk<not-in-tbl\n";
        let (out, _) = make_table(content);
        // Both empty-cell tags see the same (never-consumed) width.
        let count = out.matches("mi<tg<empty-att_<cell<width>50.00\n").count();
        assert_eq!(count, 2, "{out}");
    }

    #[test]
    fn a_pard_start_ending_a_row_definition_starts_a_row_and_cell_when_already_in_table() {
        // Exercises the `mi<mk<pard-start` branch of in_row_def state
        // without an explicit `cw<tb<row_______` -- this is the
        // branch with the confirmed `(self.__state) > 0` upstream bug
        // (see this module's doc); this test is exactly the case that
        // would otherwise never complete in the original Python.
        let content = "\
mi<mk<in-table__\n\
cw<tb<row-def___<nu<true\n\
cw<tb<cell-posit<nu<80.00\n\
mi<mk<pard-start\n\
tx<nu<__________<hi\n\
mi<mk<not-in-tbl\n";
        let (out, tables) = make_table(content);
        assert!(out.contains("mi<tg<open-att__<row"), "{out}");
        assert!(out.contains("mi<tg<open-att__<cell") || out.contains("mi<tg<open______<cell"), "{out}");
        assert_eq!(tables.len(), 1);
    }
}
