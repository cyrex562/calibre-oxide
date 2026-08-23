//! Table, row and cell formatting: reading `w:tblPr`/`w:trPr`/`w:tcPr`
//! into property sets and turning them into CSS, plus resolving a
//! `<w:tbl>`'s full row/cell/paragraph style maps and merged cells
//! ([`Table`]/[`Tables`]).
//!
//! Partial port of `old_src/src/calibre/ebooks/docx/tables.py`.
//! `Table::apply_markup`/`Tables::apply_markup` (which build the actual
//! HTML `<table>`/`<tr>`/`<td>` markup) are deferred to the same
//! follow-up as `to_html.rs`'s real port -- they need [`crate::dom`],
//! which isn't wired into the docx module yet. See issue #130.
//!
//! # `handle_merged_cells`: a tracked exclusion set, not tree mutation
//!
//! Python's `handle_merged_cells` calls `tc.getparent().remove(tc)` on
//! the *source* document tree to drop cells absorbed by a `vMerge`/
//! `hMerge` run. [`Table::removed_cells`] tracks the same set of
//! excluded `w:tc` nodes without mutating the (still read-only,
//! `roxmltree`-backed) source tree that every other property-model
//! reader in this crate depends on.
//!
//! This is checked to be behaviorally equivalent to real removal --
//! **but only downstream of `Table` itself**. Confirmed against
//! `to_html.py`'s actual pipeline: `Tables.register` (and therefore
//! `Table::__init__`/`handle_merged_cells`) runs, in full, *before*
//! the top-level paragraph-to-HTML walk even starts, and the only
//! source-tree walk over `w:tr`/`w:tc` anywhere outside `tables.py`
//! itself is `Table.apply_markup`'s own re-walk (which must honour the
//! exclusion set once ported). Critically, the top-level walk does
//! **not** check removal status -- Python's own `w:p`/`w:tbl`
//! descendant search is evaluated eagerly into a list before any
//! removal happens, so a merged-away cell's paragraph still gets HTML
//! built and appended as a stray top-level element; only
//! `apply_markup`'s fresh tree walk (which no longer finds the removed
//! `w:tc`) fails to ever move it into the table. This is a real,
//! observable upstream leak (duplicate/orphaned empty paragraphs for
//! merged-away cells), not a hypothetical -- whoever ports `to_html.rs`
//! should reproduce it (do **not** gate the top-level walk on
//! `removed_cells`) rather than silently fix it.
//!
//! # Sharing with `block_styles`
//!
//! Word tracks six border edges for tables (`left`, `top`, `right`,
//! `bottom`, `insideH`, `insideV`) instead of a paragraph's five, so
//! [`super::block_styles::Edge`]/[`super::block_styles::Borders`] grew
//! `InsideH`/`InsideV` variants/fields rather than this module
//! inventing a parallel border type — [`super::block_styles::Border`],
//! [`super::block_styles::read_border`] and
//! [`super::block_styles::border_to_css`] are reused as-is.
//!
//! # A narrower `Table::new` signature than Python's
//!
//! Python's `Table.__init__(self, namespace, tbl, styles, para_map, ...)`
//! takes the whole `Styles` collection just to call `styles.get(style_id)`
//! (a named-style lookup) -- and a shared, mutated-in-place `para_map`
//! dict threaded through every recursive sub-table construction so
//! `Tables.para_style`/`run_style` can later find which `Table` owns a
//! given paragraph. Neither needs the full (not-yet-ported) `Styles`
//! cascade orchestrator: [`Table::new`] takes `named_styles: &HashMap<String, Style>`
//! directly (`Styles::id_map`, once that type exists), and
//! [`Tables::para_style`]/[`Tables::run_style`] are backed by a flat
//! map [`Tables`] builds itself by copying each registered `Table`'s
//! (and all its nested sub-tables', recursively) resolved paragraph
//! styles out -- rather than storing `Table` references in `para_map`,
//! which Rust's ownership model makes awkward for no behavioral
//! benefit here (nothing downstream needs the *owning* `Table`, only
//! the style data it resolved).

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use roxmltree::Node;

use super::block_styles::{
    border_to_css, format_g3, pt, read_border, Borders, Css, Edge, ParagraphStyle,
};
use super::char_styles::RunStyle;
use super::names::DocxNamespace;
use super::styles::Style;

const BORDER_EDGES: [Edge; 6] = Edge::ALL_TABLE;

fn pct(value: f64) -> String {
    format!("{}%", format_g3(value))
}

// Read from XML {{{

/// A table/cell width, in the same "already-rendered CSS value" shape
/// Python uses: `"0"`, `"auto"`, `"12pt"` or `"50%"`.
///
/// Port of the Python `_read_width`.
fn read_width_value(elem: Node, ns: &DocxNamespace) -> Option<String> {
    let w: f64 = ns
        .get(elem, "w:w")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0);
    match ns.get_or(elem, "w:type", "auto") {
        "nil" => Some("0".to_string()),
        "auto" => Some("auto".to_string()),
        "dxa" => Some(pt(w / 20.0)),
        "pct" => Some(pct(w / 50.0)),
        _ => None,
    }
}

/// Port of the Python `read_width` (table width, from `w:tblW`).
fn read_width(parent: Node, ns: &DocxNamespace) -> Option<String> {
    ns.children(parent, &["w:tblW"])
        .into_iter()
        .last()
        .and_then(|w| read_width_value(w, ns))
}

/// Port of the Python `read_cell_width` (from `w:tcW`).
fn read_cell_width(parent: Node, ns: &DocxNamespace) -> Option<String> {
    ns.children(parent, &["w:tcW"])
        .into_iter()
        .last()
        .and_then(|w| read_width_value(w, ns))
}

/// The four `cell_padding_*` values read from a `tblCellMar`/`tcMar`
/// container.
///
/// Port of the Python `read_padding`.
#[derive(Debug, Clone, Default)]
struct CellPadding {
    left: Option<String>,
    top: Option<String>,
    right: Option<String>,
    bottom: Option<String>,
}

fn read_padding(parent: Node, ns: &DocxNamespace, container_name: &str) -> CellPadding {
    let mut ans = CellPadding::default();
    for mar in ns.children(parent, &[&format!("w:{container_name}")]) {
        if let Some(edge) = ns.children(mar, &["w:left"]).into_iter().last() {
            ans.left = read_width_value(edge, ns);
        }
        if let Some(edge) = ns.children(mar, &["w:top"]).into_iter().last() {
            ans.top = read_width_value(edge, ns);
        }
        if let Some(edge) = ns.children(mar, &["w:right"]).into_iter().last() {
            ans.right = read_width_value(edge, ns);
        }
        if let Some(edge) = ns.children(mar, &["w:bottom"]).into_iter().last() {
            ans.bottom = read_width_value(edge, ns);
        }
    }
    ans
}

/// Table-level justification: unlike a paragraph's `w:jc`, this only
/// ever sets the table's left/right auto-margins (`"left"` -> right
/// margin auto, `"right"` -> left margin auto, `"center"` -> both).
///
/// Port of the Python `read_justification` (the `tables.py` one).
fn read_table_justification(parent: Node, ns: &DocxNamespace) -> (Option<String>, Option<String>) {
    let (mut left, mut right) = (None, None);
    for jc in ns.children(parent, &["w:jc"]) {
        let Some(val) = ns.get(jc, "w:val").filter(|v| !v.is_empty()) else {
            continue;
        };
        match val {
            "left" => right = Some("auto".to_string()),
            "right" => left = Some("auto".to_string()),
            "center" => {
                left = Some("auto".to_string());
                right = Some("auto".to_string());
            }
            _ => {}
        }
    }
    (left, right)
}

/// Port of the Python `read_spacing` (`w:tblCellSpacing`, read from
/// either a `tblPr` or a `trPr` -- shared between [`TableStyle`] and
/// [`RowStyle`]).
fn read_spacing(parent: Node, ns: &DocxNamespace) -> Option<String> {
    ns.children(parent, &["w:tblCellSpacing"])
        .into_iter()
        .last()
        .and_then(|cs| read_width_value(cs, ns))
}

/// The table's floating-position attributes (`w:tblpPr`), kept as a
/// flat local-name -> value map, the same shape Python's
/// dict-comprehension over `.attrib.items()` produces.
///
/// Port of the Python `read_float`.
fn read_float(parent: Node, ns: &DocxNamespace) -> Option<IndexMap<String, String>> {
    let x = ns.children(parent, &["w:tblpPr"]).into_iter().last()?;
    let mut ans = IndexMap::new();
    for attr in x.attributes() {
        ans.insert(attr.name().to_string(), attr.value().to_string());
    }
    Some(ans)
}

/// Port of the Python `read_indent` (`w:tblInd`).
fn read_table_indent(parent: Node, ns: &DocxNamespace) -> Option<String> {
    ns.children(parent, &["w:tblInd"])
        .into_iter()
        .last()
        .and_then(|cs| read_width_value(cs, ns))
}

/// Port of the Python `read_borders`: chooses `tblBorders`/`tcBorders`
/// by the caller's context, unlike paragraphs which always read
/// `pBdr`.
fn read_table_borders(parent: Node, ns: &DocxNamespace, container_name: &str) -> Borders {
    read_border(parent, ns, container_name, &BORDER_EDGES)
}

/// The row height rule and value, from `w:trHeight`.
///
/// Port of the Python `read_height`.
#[derive(Debug, Clone, PartialEq)]
struct RowHeight {
    rule: String,
    val: Option<String>,
}

fn read_height(parent: Node, ns: &DocxNamespace) -> Option<RowHeight> {
    let mut ans = None;
    for rh in ns.children(parent, &["w:trHeight"]) {
        let rule = ns.get_or(rh, "w:hRule", "auto");
        if matches!(rule, "auto" | "atLeast" | "exact") {
            ans = Some(RowHeight {
                rule: rule.to_string(),
                val: ns.get(rh, "w:val").map(|v| v.to_string()),
            });
        }
    }
    ans
}

/// Port of the Python `read_vertical_align` (`w:vAlign`).
fn read_vertical_align(parent: Node, ns: &DocxNamespace) -> Option<String> {
    let mut ans = None;
    for va in ns.children(parent, &["w:vAlign"]) {
        let val = ns.get(va, "w:val");
        ans = Some(
            match val {
                Some("center") => "middle",
                Some("top") => "top",
                Some("bottom") => "bottom",
                _ => "middle",
            }
            .to_string(),
        );
    }
    ans
}

/// Port of the Python `read_col_span` (`w:gridSpan`).
fn read_col_span(parent: Node, ns: &DocxNamespace) -> Option<i64> {
    let mut ans = None;
    for gs in ns.children(parent, &["w:gridSpan"]) {
        if let Some(v) = ns.get(gs, "w:val").and_then(|v| v.trim().parse().ok()) {
            ans = Some(v);
        }
    }
    ans
}

/// Port of the Python `read_merge`: `hMerge`/`vMerge`, `"continue"`
/// when the element is present without an explicit `w:val`.
fn read_merge(parent: Node, ns: &DocxNamespace, name: &str) -> Option<String> {
    let mut ans = None;
    for m in ns.children(parent, &[&format!("w:{name}")]) {
        ans = Some(ns.get_or(m, "w:val", "continue").to_string());
    }
    ans
}

/// Port of the Python `read_band_size`: `w:tblStyleColBandSize` /
/// `w:tblStyleRowBandSize`, defaulting to `1` (not inheriting -- these
/// are never `inherit` in Python either).
fn read_band_size(parent: Node, ns: &DocxNamespace, name: &str) -> i64 {
    let mut ans = 1;
    for y in ns.children(parent, &[&format!("w:tblStyle{name}BandSize")]) {
        if let Some(v) = ns.get(y, "w:val").and_then(|v| v.trim().parse().ok()) {
            ans = v;
        }
    }
    ans
}

/// Port of the Python `read_look` (`w:tblLook`, a hex bitmask).
fn read_look(parent: Node, ns: &DocxNamespace) -> i64 {
    let mut ans = 0;
    for x in ns.children(parent, &["w:tblLook"]) {
        if let Some(v) = ns
            .get(x, "w:val")
            .and_then(|v| i64::from_str_radix(v.trim(), 16).ok())
        {
            ans = v;
        }
    }
    ans
}

// }}}

/// Border + padding CSS shared by [`CellStyle`] and [`TableStyle`].
///
/// Port of the Python `Style.convert_border`.
fn convert_border_css(borders: &Borders, is_bidi: bool) -> Css {
    let mut c = Css::new();
    for edge in Edge::CSS_EDGES {
        border_to_css(edge, borders, &mut c);
        if let Some(padding) = borders.edge(edge).padding {
            c.insert(format!("padding-{}", edge.as_str()), pt(padding));
        }
    }
    if is_bidi {
        for base in [
            "padding-%s",
            "border-%s-style",
            "border-%s-color",
            "border-%s-width",
        ] {
            let l = c.get(&base.replace("%s", "left")).cloned();
            let r = c.get(&base.replace("%s", "right")).cloned();
            if let Some(l) = l {
                c.insert(base.replace("%s", "right"), l);
            }
            if let Some(r) = r {
                c.insert(base.replace("%s", "left"), r);
            }
        }
    }
    c
}

/// `border-collapse`/`border-spacing` CSS shared by [`RowStyle`] and
/// [`TableStyle`].
///
/// Port of the Python `Style.convert_spacing`.
fn convert_spacing_css(spacing: &Option<String>) -> Css {
    let mut c = Css::new();
    if let Some(spacing) = spacing {
        if matches!(spacing.as_str(), "auto" | "0") {
            c.insert("border-collapse".to_string(), "collapse".to_string());
        } else {
            c.insert("border-collapse".to_string(), "separate".to_string());
            c.insert("border-spacing".to_string(), spacing.clone());
        }
    }
    c
}

/// A `<w:tr>`'s resolved formatting.
///
/// Port of the Python `RowStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowStyle {
    height: Option<(String, Option<String>)>,
    pub cant_split: Option<bool>,
    pub hidden: Option<bool>,
    pub spacing: Option<String>,
    pub is_bidi: bool,
}

impl RowStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `RowStyle(namespace, trPr)`.
    pub fn from_trpr(trpr: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::new();
        s.hidden = super::block_styles::binary_property(trpr, "hidden", ns);
        s.cant_split = super::block_styles::binary_property(trpr, "cantSplit", ns);
        s.spacing = read_spacing(trpr, ns);
        s.height = read_height(trpr, ns).map(|h| (h.rule, h.val));
        s
    }

    pub fn apply_bidi(&mut self) {
        self.is_bidi = true;
    }

    /// Port of the Python `RowStyle.update`.
    pub fn update(&mut self, other: &RowStyle) {
        if other.height.is_some() {
            self.height.clone_from(&other.height);
        }
        if other.cant_split.is_some() {
            self.cant_split = other.cant_split;
        }
        if other.hidden.is_some() {
            self.hidden = other.hidden;
        }
        if other.spacing.is_some() {
            self.spacing.clone_from(&other.spacing);
        }
    }

    /// Port of the Python `RowStyle.css`.
    pub fn css(&self) -> Css {
        let mut c = Css::new();
        if self.hidden == Some(true) {
            c.insert("display".to_string(), "none".to_string());
        }
        if self.cant_split == Some(true) {
            c.insert("page-break-inside".to_string(), "avoid".to_string());
        }
        if let Some((rule, val)) = &self.height {
            if rule != "auto" {
                if let Some(v) = val.as_deref().and_then(|v| v.trim().parse::<f64>().ok()) {
                    let key = if rule == "atLeast" {
                        "min-height"
                    } else {
                        "height"
                    };
                    c.insert(key.to_string(), pt(v / 20.0));
                }
            }
        }
        c.extend(convert_spacing_css(&self.spacing));
        c
    }
}

/// A `<w:tc>`'s resolved formatting.
///
/// Port of the Python `CellStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellStyle {
    pub background_color: Option<String>,
    cell_padding_left: Option<String>,
    cell_padding_right: Option<String>,
    cell_padding_top: Option<String>,
    cell_padding_bottom: Option<String>,
    pub width: Option<String>,
    pub vertical_align: Option<String>,
    pub col_span: Option<i64>,
    pub v_merge: Option<String>,
    pub h_merge: Option<String>,
    pub row_span: Option<i64>,
    pub borders: Borders,
    pub is_bidi: bool,
}

impl CellStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `CellStyle(namespace, tcPr)`.
    pub fn from_tcpr(tcpr: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::new();
        s.borders = read_table_borders(tcpr, ns, "tcBorders");
        s.background_color = super::block_styles::read_shd(tcpr, ns);
        let padding = read_padding(tcpr, ns, "tcMar");
        s.cell_padding_left = padding.left;
        s.cell_padding_top = padding.top;
        s.cell_padding_right = padding.right;
        s.cell_padding_bottom = padding.bottom;
        s.width = read_cell_width(tcpr, ns);
        s.vertical_align = read_vertical_align(tcpr, ns);
        s.col_span = read_col_span(tcpr, ns);
        s.h_merge = read_merge(tcpr, ns, "hMerge");
        s.v_merge = read_merge(tcpr, ns, "vMerge");
        // `row_span` is never read from XML directly -- it is only
        // ever computed by merged-cell handling (`Table`, deferred).
        s.row_span = None;
        s
    }

    fn cell_padding(&self, edge: Edge) -> &Option<String> {
        match edge {
            Edge::Left => &self.cell_padding_left,
            Edge::Top => &self.cell_padding_top,
            Edge::Right => &self.cell_padding_right,
            Edge::Bottom => &self.cell_padding_bottom,
            Edge::Between | Edge::InsideH | Edge::InsideV => &None,
        }
    }

    fn cell_padding_mut(&mut self, edge: Edge) -> &mut Option<String> {
        match edge {
            Edge::Left => &mut self.cell_padding_left,
            Edge::Top => &mut self.cell_padding_top,
            Edge::Right => &mut self.cell_padding_right,
            Edge::Bottom => &mut self.cell_padding_bottom,
            Edge::Between | Edge::InsideH | Edge::InsideV => {
                unreachable!("cell padding is only ever CSS_EDGES")
            }
        }
    }

    pub fn apply_bidi(&mut self) {
        self.is_bidi = true;
    }

    /// Port of the Python `CellStyle.update`.
    pub fn update(&mut self, other: &CellStyle) {
        if other.background_color.is_some() {
            self.background_color.clone_from(&other.background_color);
        }
        for edge in Edge::CSS_EDGES {
            if other.cell_padding(edge).is_some() {
                self.cell_padding_mut(edge)
                    .clone_from(other.cell_padding(edge));
            }
        }
        if other.width.is_some() {
            self.width.clone_from(&other.width);
        }
        if other.vertical_align.is_some() {
            self.vertical_align.clone_from(&other.vertical_align);
        }
        if other.col_span.is_some() {
            self.col_span = other.col_span;
        }
        if other.v_merge.is_some() {
            self.v_merge.clone_from(&other.v_merge);
        }
        if other.h_merge.is_some() {
            self.h_merge.clone_from(&other.h_merge);
        }
        if other.row_span.is_some() {
            self.row_span = other.row_span;
        }
        for edge in Edge::ALL_TABLE {
            let src = other.borders.edge(edge).clone();
            let dest = self.borders.edge_mut(edge);
            if src.color.is_some() {
                dest.color = src.color;
            }
            if src.style.is_some() {
                dest.style = src.style;
            }
            if src.width.is_some() {
                dest.width = src.width;
            }
            if src.padding.is_some() {
                dest.padding = src.padding;
            }
        }
    }

    /// Port of the Python `CellStyle.css`.
    pub fn css(&self) -> Css {
        let mut c = Css::new();
        if let Some(bg) = &self.background_color {
            c.insert("background-color".to_string(), bg.clone());
        }
        if let Some(w) = self.width.as_deref().filter(|w| *w != "auto") {
            c.insert("width".to_string(), w.to_string());
        }
        c.insert(
            "vertical-align".to_string(),
            self.vertical_align.clone().unwrap_or("top".to_string()),
        );
        for edge in Edge::CSS_EDGES {
            let name = edge.as_str();
            match self.cell_padding(edge).as_deref() {
                Some(v) if v != "auto" => {
                    c.insert(format!("padding-{name}"), v.to_string());
                }
                None if matches!(edge, Edge::Left | Edge::Right) => {
                    c.insert(format!("padding-{name}"), pt(115.0 / 20.0));
                }
                _ => {}
            }
        }
        for x in ["top", "bottom"] {
            let key = format!("padding-{x}");
            if c.get(&key).map(String::as_str).unwrap_or("0pt") == "0pt" {
                c.insert(key, "0.5ex".to_string());
            }
        }
        c.extend(convert_border_css(&self.borders, self.is_bidi));
        c
    }
}

/// An entry in [`TableStyle::overrides`] -- the conditional formatting
/// `w:tblStylePr` attaches for e.g. the first row or banded columns.
///
/// Port of the per-`otype` dict Python builds inline in `TableStyle.__init__`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableStyleOverride {
    pub table: Option<Box<TableStyle>>,
    pub row: Option<RowStyle>,
    pub cell: Option<CellStyle>,
    pub para: Option<ParagraphStyle>,
    pub run: Option<RunStyle>,
}

/// A `<w:tbl>`'s resolved formatting, including any named-style
/// conditional-formatting overrides (`w:tblStylePr`).
///
/// Port of the Python `TableStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableStyle {
    pub width: Option<String>,
    pub float: Option<IndexMap<String, String>>,
    cell_padding_left: Option<String>,
    cell_padding_right: Option<String>,
    cell_padding_top: Option<String>,
    cell_padding_bottom: Option<String>,
    pub margin_left: Option<String>,
    pub margin_right: Option<String>,
    pub background_color: Option<String>,
    pub spacing: Option<String>,
    pub indent: Option<String>,
    /// `None` means inherit (Python's `inherit`); `Some(map)` means
    /// resolved -- possibly to an empty map, when this style has no
    /// conditional formatting of its own.
    pub overrides: Option<IndexMap<String, TableStyleOverride>>,
    pub col_band_size: i64,
    pub row_band_size: i64,
    pub look: i64,
    pub bidi: Option<bool>,
    pub borders: Borders,
}

impl TableStyle {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.col_band_size = 1;
        s.row_band_size = 1;
        s
    }

    /// Port of `TableStyle(namespace, tblPr)`.
    pub fn from_tblpr(tblpr: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::new();
        s.bidi = super::block_styles::binary_property(tblpr, "bidiVisual", ns);
        s.width = read_width(tblpr, ns);
        s.float = read_float(tblpr, ns);
        let padding = read_padding(tblpr, ns, "tblCellMar");
        s.cell_padding_left = padding.left;
        s.cell_padding_top = padding.top;
        s.cell_padding_right = padding.right;
        s.cell_padding_bottom = padding.bottom;
        s.background_color = super::block_styles::read_shd(tblpr, ns);
        let (l, r) = read_table_justification(tblpr, ns);
        s.margin_left = l;
        s.margin_right = r;
        s.spacing = read_spacing(tblpr, ns);
        s.indent = read_table_indent(tblpr, ns);
        s.borders = read_table_borders(tblpr, ns, "tblBorders");
        s.col_band_size = read_band_size(tblpr, ns, "Col");
        s.row_band_size = read_band_size(tblpr, ns, "Row");
        s.look = read_look(tblpr, ns);

        if let Some(parent) = tblpr.parent_element() {
            if ns.is_tag(parent, "w:style") {
                let mut overrides = IndexMap::new();
                for tbl_style_pr in ns.children(parent, &["w:tblStylePr"]) {
                    let Some(otype) = ns.get(tbl_style_pr, "w:type") else {
                        continue;
                    };
                    let mut ovr = TableStyleOverride::default();
                    for t in ns.children(tbl_style_pr, &["w:tblPr"]) {
                        ovr.table = Some(Box::new(TableStyle::from_tblpr(t, ns)));
                    }
                    for t in ns.children(tbl_style_pr, &["w:trPr"]) {
                        ovr.row = Some(RowStyle::from_trpr(t, ns));
                    }
                    for t in ns.children(tbl_style_pr, &["w:tcPr"]) {
                        ovr.cell = Some(CellStyle::from_tcpr(t, ns));
                    }
                    for t in ns.children(tbl_style_pr, &["w:pPr"]) {
                        ovr.para = Some(ParagraphStyle::from_ppr(t, ns));
                    }
                    for t in ns.children(tbl_style_pr, &["w:rPr"]) {
                        ovr.run = Some(RunStyle::from_rpr(t, ns));
                    }
                    overrides.insert(otype.to_string(), ovr);
                }
                s.overrides = Some(overrides);
            }
        }
        s
    }

    pub fn apply_bidi(&mut self) {
        self.bidi = Some(true);
    }

    /// The cell-padding fallback [`Table::resolve_cell_style`] uses
    /// when a cell doesn't specify its own.
    fn cell_padding(&self, edge: Edge) -> &Option<String> {
        match edge {
            Edge::Left => &self.cell_padding_left,
            Edge::Top => &self.cell_padding_top,
            Edge::Right => &self.cell_padding_right,
            Edge::Bottom => &self.cell_padding_bottom,
            Edge::Between | Edge::InsideH | Edge::InsideV => &None,
        }
    }

    /// Port of the Python `TableStyle.update`.
    pub fn update(&mut self, other: &TableStyle) {
        macro_rules! overlay {
            ($($field:ident),* $(,)?) => {
                $(if other.$field.is_some() { self.$field.clone_from(&other.$field); })*
            };
        }
        overlay!(
            width,
            float,
            cell_padding_left,
            cell_padding_top,
            cell_padding_right,
            cell_padding_bottom,
            margin_left,
            margin_right,
            background_color,
            spacing,
            indent,
            overrides,
            bidi,
        );
        // `col_band_size`/`row_band_size`/`look` are never `inherit`
        // in Python -- an unset `TableStyle(namespace)` still carries
        // their real defaults, which `update` always overwrites.
        self.col_band_size = other.col_band_size;
        self.row_band_size = other.row_band_size;
        self.look = other.look;
        for edge in Edge::ALL_TABLE {
            let src = other.borders.edge(edge).clone();
            let dest = self.borders.edge_mut(edge);
            if src.color.is_some() {
                dest.color = src.color;
            }
            if src.style.is_some() {
                dest.style = src.style;
            }
            if src.width.is_some() {
                dest.width = src.width;
            }
            if src.padding.is_some() {
                dest.padding = src.padding;
            }
        }
    }

    /// Port of the Python `TableStyle.resolve_based_on`.
    pub fn resolve_based_on(&mut self, parent: &TableStyle) {
        if self.width.is_none() {
            self.width.clone_from(&parent.width);
        }
        if self.float.is_none() {
            self.float.clone_from(&parent.float);
        }
        if self.cell_padding_left.is_none() {
            self.cell_padding_left.clone_from(&parent.cell_padding_left);
        }
        if self.cell_padding_top.is_none() {
            self.cell_padding_top.clone_from(&parent.cell_padding_top);
        }
        if self.cell_padding_right.is_none() {
            self.cell_padding_right
                .clone_from(&parent.cell_padding_right);
        }
        if self.cell_padding_bottom.is_none() {
            self.cell_padding_bottom
                .clone_from(&parent.cell_padding_bottom);
        }
        if self.margin_left.is_none() {
            self.margin_left.clone_from(&parent.margin_left);
        }
        if self.margin_right.is_none() {
            self.margin_right.clone_from(&parent.margin_right);
        }
        if self.background_color.is_none() {
            self.background_color.clone_from(&parent.background_color);
        }
        if self.spacing.is_none() {
            self.spacing.clone_from(&parent.spacing);
        }
        if self.indent.is_none() {
            self.indent.clone_from(&parent.indent);
        }
        if self.overrides.is_none() {
            self.overrides.clone_from(&parent.overrides);
        }
        if self.bidi.is_none() {
            self.bidi = parent.bidi;
        }
        self.col_band_size = parent.col_band_size;
        self.row_band_size = parent.row_band_size;
        self.look = parent.look;
        for edge in Edge::ALL_TABLE {
            let src = parent.borders.edge(edge).clone();
            let dest = self.borders.edge_mut(edge);
            if dest.color.is_none() {
                dest.color = src.color;
            }
            if dest.style.is_none() {
                dest.style = src.style;
            }
            if dest.width.is_none() {
                dest.width = src.width;
            }
            if dest.padding.is_none() {
                dest.padding = src.padding;
            }
        }
    }

    /// Port of the Python `TableStyle.css`. `page` supplies the page
    /// dimensions `float`'s `tblpX`-relative-to-page-width heuristic
    /// needs; it is only consulted when `float` has no explicit
    /// `tblpXSpec`.
    pub fn css(&self, page: &super::styles::PageProperties) -> Css {
        let mut c = Css::new();
        if let Some(w) = self.width.as_deref().filter(|w| *w != "auto") {
            c.insert("width".to_string(), w.to_string());
        }
        if let Some(v) = &self.background_color {
            c.insert("background-color".to_string(), v.clone());
        }
        if let Some(v) = &self.margin_left {
            c.insert("margin-left".to_string(), v.clone());
        }
        if let Some(v) = &self.margin_right {
            c.insert("margin-right".to_string(), v.clone());
        }
        if let Some(indent) = self.indent.as_deref().filter(|v| *v != "auto") {
            if self.margin_left.as_deref() != Some("auto") {
                c.insert("margin-left".to_string(), indent.to_string());
            }
        }
        if let Some(float) = &self.float {
            for x in ["left", "top", "right", "bottom"] {
                let val = float
                    .get(&format!("{x}FromText"))
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .map(|v| pt(v / 20.0))
                    .unwrap_or("0".to_string());
                c.insert(format!("margin-{x}"), val);
            }
            if let Some(spec) = float.get("tblpXSpec") {
                c.insert(
                    "float".to_string(),
                    if matches!(spec.as_str(), "right" | "outside") {
                        "right"
                    } else {
                        "left"
                    }
                    .to_string(),
                );
            } else {
                let page_width = page.width - page.margin_left - page.margin_right;
                let x = float
                    .get("tblpX")
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .unwrap_or(0.0)
                    / 20.0;
                let float_val = if page_width != 0.0 && (x / page_width) < 0.65 {
                    "left"
                } else {
                    "right"
                };
                c.insert("float".to_string(), float_val.to_string());
            }
        }
        c.extend(convert_spacing_css(&self.spacing));
        if !c.contains_key("border-collapse") {
            c.insert("border-collapse".to_string(), "collapse".to_string());
        }
        c.extend(convert_border_css(&self.borders, self.bidi == Some(true)));
        c
    }
}

/// A `<w:tbl>`'s resolved row/cell/paragraph style maps and merged-cell
/// bookkeeping. See the module docs for what's deferred (`apply_markup`)
/// and how merged-cell removal is represented here.
///
/// Port of the Python `Table`.
#[derive(Debug, Clone)]
pub struct Table<'a, 'i> {
    pub tbl: Node<'a, 'i>,
    pub is_sub_table: bool,
    pub table_style: TableStyle,
    pub paragraph_style: Option<ParagraphStyle>,
    pub run_style: Option<RunStyle>,
    overrides: IndexMap<String, TableStyleOverride>,
    pub style_map_row: HashMap<Node<'a, 'i>, RowStyle>,
    pub style_map_cell: HashMap<Node<'a, 'i>, CellStyle>,
    pub style_map_para: HashMap<Node<'a, 'i>, (Option<ParagraphStyle>, Option<RunStyle>)>,
    pub paragraphs: Vec<Node<'a, 'i>>,
    pub cell_map: Vec<Vec<Node<'a, 'i>>>,
    pub sub_tables: HashMap<Node<'a, 'i>, Table<'a, 'i>>,
    /// `w:tc` nodes absorbed by a `vMerge`/`hMerge` run -- see the
    /// module docs' "tracked exclusion set, not tree mutation" section.
    pub removed_cells: HashSet<Node<'a, 'i>>,
}

impl<'a, 'i> Table<'a, 'i> {
    /// Port of `Table(namespace, tbl, styles, para_map, is_sub_table)`.
    /// See the module docs for why this takes `named_styles` directly
    /// rather than the whole (not yet ported) `Styles` collection, and
    /// why there is no `para_map` parameter.
    pub fn new(
        tbl: Node<'a, 'i>,
        named_styles: &HashMap<String, Style>,
        ns: &DocxNamespace,
        is_sub_table: bool,
    ) -> Table<'a, 'i> {
        let mut table_style = TableStyle::new();
        let mut paragraph_style: Option<ParagraphStyle> = None;
        let mut run_style: Option<RunStyle> = None;

        for tblpr in ns.children(tbl, &["w:tblPr"]) {
            for ts in ns.children(tblpr, &["w:tblStyle"]) {
                if let Some(style_id) = ns.get(ts, "w:val") {
                    if let Some(s) = named_styles.get(style_id) {
                        if let Some(t) = &s.table_style {
                            table_style.update(t);
                        }
                        if let Some(p) = &s.paragraph_style {
                            match &mut paragraph_style {
                                None => paragraph_style = Some(p.clone()),
                                Some(existing) => existing.update(p),
                            }
                        }
                        if let Some(c) = &s.character_style {
                            match &mut run_style {
                                None => run_style = Some(c.clone()),
                                Some(existing) => existing.update(c),
                            }
                        }
                    }
                }
            }
            table_style.update(&TableStyle::from_tblpr(tblpr, ns));
        }

        let overrides = table_style.overrides.clone().unwrap_or_default();
        if let Some(whole) = overrides.get("wholeTable") {
            if let Some(t) = whole.table.clone() {
                table_style.update(&t);
            }
        }

        let mut table = Table {
            tbl,
            is_sub_table,
            table_style,
            paragraph_style,
            run_style,
            overrides,
            style_map_row: HashMap::new(),
            style_map_cell: HashMap::new(),
            style_map_para: HashMap::new(),
            paragraphs: Vec::new(),
            cell_map: Vec::new(),
            sub_tables: HashMap::new(),
            removed_cells: HashSet::new(),
        };

        let rows = ns.children(tbl, &["w:tr"]);
        let num_rows = rows.len();
        for (r, &tr) in rows.iter().enumerate() {
            let row_overrides = table.get_overrides(r, None, num_rows, None);
            table.resolve_row_style(tr, &row_overrides, ns);
            let cells = ns.children(tr, &["w:tc"]);
            let num_cols = cells.len();
            let mut row_cells = Vec::new();
            for (c, &tc) in cells.iter().enumerate() {
                let cell_overrides = table.get_overrides(r, Some(c), num_rows, Some(num_cols));
                table.resolve_cell_style(tc, &cell_overrides, r, c, num_rows, num_cols, ns);
                row_cells.push(tc);
                for p in ns.children(tc, &["w:p"]) {
                    table.paragraphs.push(p);
                    table.resolve_para_style(p, &cell_overrides);
                }
            }
            table.cell_map.push(row_cells);
        }

        table.handle_merged_cells();

        // Port of `./w:tr/w:tc/w:tbl` -- direct nested tables only, one
        // level relative to `tbl`. Deliberately not a full descendant
        // search: a table nested two or more levels deep is not in
        // *this* table's `sub_tables` (Python's own XPath scope is the
        // same one level), only in its immediate parent's -- see
        // `Tables::register`'s docs for why that matters.
        for &tr in &rows {
            for tc in ns.children(tr, &["w:tc"]) {
                for sub_tbl in ns.children(tc, &["w:tbl"]) {
                    let sub = Table::new(sub_tbl, named_styles, ns, true);
                    table.sub_tables.insert(sub_tbl, sub);
                }
            }
        }

        table
    }

    fn bidi(&self) -> bool {
        self.table_style.bidi == Some(true)
    }

    /// Port of the Python `Table.override_allowed`.
    fn override_allowed(&self, name: &str) -> bool {
        if name.ends_with("Cell") || name == "wholeTable" {
            return true;
        }
        let look = self.table_style.look;
        if (look & 0x0020 != 0 && name == "firstRow")
            || (look & 0x0040 != 0 && name == "lastRow")
            || (look & 0x0080 != 0 && name == "firstCol")
            || (look & 0x0100 != 0 && name == "lastCol")
        {
            return true;
        }
        if let Some(suffix) = name.strip_prefix("band") {
            if suffix.ends_with("Horz") {
                return look & 0x0200 == 0;
            }
            if suffix.ends_with("Vert") {
                return look & 0x0400 == 0;
            }
        }
        false
    }

    /// Port of the Python `Table.get_overrides`.
    fn get_overrides(
        &self,
        r: usize,
        c: Option<usize>,
        num_of_rows: usize,
        num_of_cols_in_row: Option<usize>,
    ) -> Vec<String> {
        // Python's `(m - (m % n)) // n` is plain floor division for
        // the non-negative operands real documents produce; guarded
        // against a malformed `band_size` of 0 (which Python would
        // raise `ZeroDivisionError` on) rather than reproduced, since
        // that's an input-validation concern, not an author-intended
        // quirk.
        fn divisor(m: usize, n: i64) -> i64 {
            m as i64 / n.max(1)
        }

        let mut overrides = vec!["wholeTable".to_string()];
        if let Some(c) = c {
            let odd_column_band = divisor(c, self.table_style.col_band_size) % 2 == 1;
            overrides.push(format!("band{}Vert", if odd_column_band { 1 } else { 2 }));
        }
        let odd_row_band = divisor(r, self.table_style.row_band_size) % 2 == 1;
        overrides.push(format!("band{}Horz", if odd_row_band { 1 } else { 2 }));

        if let Some(c) = c {
            if c == 0 {
                overrides.push("firstCol".to_string());
            }
            if let Some(n) = num_of_cols_in_row {
                if c + 1 >= n {
                    overrides.push("lastCol".to_string());
                }
            }
        }
        if r == 0 {
            overrides.push("firstRow".to_string());
        }
        if r + 1 >= num_of_rows {
            overrides.push("lastRow".to_string());
        }
        if let Some(c) = c {
            if r == 0 {
                if c == 0 {
                    overrides.push("nwCell".to_string());
                }
                if num_of_cols_in_row == Some(c + 1) {
                    overrides.push("neCell".to_string());
                }
            }
            if r + 1 == num_of_rows {
                if c == 0 {
                    overrides.push("swCell".to_string());
                }
                if num_of_cols_in_row == Some(c + 1) {
                    overrides.push("seCell".to_string());
                }
            }
        }

        overrides
            .into_iter()
            .filter(|o| self.override_allowed(o))
            .collect()
    }

    /// Port of the Python `Table.resolve_row_style`.
    fn resolve_row_style(&mut self, tr: Node<'a, 'i>, overrides: &[String], ns: &DocxNamespace) {
        let mut rs = RowStyle::new();
        for o in overrides {
            if let Some(ovr) = self.overrides.get(o) {
                if let Some(ors) = &ovr.row {
                    rs.update(ors);
                }
            }
        }
        for trpr in ns.children(tr, &["w:trPr"]) {
            rs.update(&RowStyle::from_trpr(trpr, ns));
        }
        if self.bidi() {
            rs.apply_bidi();
        }
        self.style_map_row.insert(tr, rs);
    }

    /// Port of the Python `Table.resolve_cell_style`.
    #[allow(clippy::too_many_arguments)]
    fn resolve_cell_style(
        &mut self,
        tc: Node<'a, 'i>,
        overrides: &[String],
        row: usize,
        col: usize,
        rows: usize,
        cols_in_row: usize,
        ns: &DocxNamespace,
    ) {
        let mut cs = CellStyle::new();
        for o in overrides {
            if let Some(ovr) = self.overrides.get(o) {
                if let Some(ors) = &ovr.cell {
                    cs.update(ors);
                }
            }
        }
        for tcpr in ns.children(tc, &["w:tcPr"]) {
            cs.update(&CellStyle::from_tcpr(tcpr, ns));
        }

        for edge in Edge::CSS_EDGES {
            if cs.cell_padding(edge).is_none() {
                let fallback = self.table_style.cell_padding(edge).clone();
                *cs.cell_padding_mut(edge) = fallback;
            }

            let is_inside_edge = match edge {
                Edge::Left => col > 0,
                Edge::Top => row > 0,
                Edge::Right => col + 1 < cols_in_row,
                Edge::Bottom => row + 1 < rows,
                _ => false,
            };
            let inside_edge = is_inside_edge.then(|| {
                if matches!(edge, Edge::Top | Edge::Bottom) {
                    Edge::InsideH
                } else {
                    Edge::InsideV
                }
            });

            if cs.borders.edge(edge).color.is_none() {
                if let Some(inside) = inside_edge {
                    let v = cs
                        .borders
                        .edge(inside)
                        .color
                        .clone()
                        .or_else(|| self.table_style.borders.edge(inside).color.clone());
                    cs.borders.edge_mut(edge).color = v;
                }
            }

            if cs.borders.edge(edge).style.is_none() {
                if let Some(inside) = inside_edge {
                    let v = cs
                        .borders
                        .edge(inside)
                        .style
                        .clone()
                        .or_else(|| self.table_style.borders.edge(inside).style.clone());
                    cs.borders.edge_mut(edge).style = v;
                }
            }
            if !is_inside_edge && cs.borders.edge(edge).style.as_deref() == Some("none") {
                cs.borders.edge_mut(edge).style = Some("hidden".to_string());
            }

            if cs.borders.edge(edge).width.is_none() {
                if let Some(inside) = inside_edge {
                    let v = cs.borders.edge(inside).width.or(self
                        .table_style
                        .borders
                        .edge(inside)
                        .width);
                    cs.borders.edge_mut(edge).width = v;
                }
            }
        }

        if self.bidi() {
            cs.apply_bidi();
        }
        self.style_map_cell.insert(tc, cs);
    }

    /// Port of the Python `Table.resolve_para_style`.
    fn resolve_para_style(&mut self, p: Node<'a, 'i>, overrides: &[String]) {
        let mut para = self.paragraph_style.clone();
        let mut run = self.run_style.clone();
        for o in overrides {
            if let Some(ovr) = self.overrides.get(o) {
                if let Some(ops) = &ovr.para {
                    match &mut para {
                        None => para = Some(ops.clone()),
                        Some(existing) => existing.update(ops),
                    }
                }
                if let Some(ops) = &ovr.run {
                    match &mut run {
                        None => run = Some(ops.clone()),
                        Some(existing) => existing.update(ops),
                    }
                }
            }
        }
        self.style_map_para.insert(p, (para, run));
    }

    /// Marks cells absorbed by a `vMerge`/`hMerge` run as removed
    /// (added to [`Table::removed_cells`]) and records `row_span`/
    /// `col_span` on the surviving cell's [`CellStyle`]. See the
    /// module docs for why this doesn't mutate the source tree.
    ///
    /// Port of the Python `Table.handle_merged_cells`.
    fn handle_merged_cells(&mut self) {
        if self.cell_map.is_empty() {
            return;
        }

        // Vertical merges (vMerge / row_span), column by column.
        let max_col_num = self.cell_map.iter().map(|r| r.len()).max().unwrap_or(0);
        for c in 0..max_col_num {
            let cells: Vec<Option<Node<'a, 'i>>> = self
                .cell_map
                .iter()
                .map(|row| row.get(c).copied())
                .collect();
            let mut runs: Vec<Vec<Node<'a, 'i>>> = vec![Vec::new()];
            for cell in cells {
                let s = cell
                    .and_then(|c| self.style_map_cell.get(&c))
                    .cloned()
                    .unwrap_or_default();
                match (cell, s.v_merge.as_deref()) {
                    (Some(c), Some("restart")) => runs.push(vec![c]),
                    (Some(c), Some("continue")) => {
                        if let Some(last) = runs.last_mut() {
                            last.push(c);
                        }
                    }
                    _ => runs.push(Vec::new()),
                }
            }
            self.commit_merge_run(runs, |cs, len| cs.row_span = Some(len));
        }

        // Horizontal merges (hMerge / col_span), row by row.
        for cells in self.cell_map.clone() {
            let mut runs: Vec<Vec<Node<'a, 'i>>> = vec![Vec::new()];
            for cell in cells {
                let s = self.style_map_cell.get(&cell).cloned().unwrap_or_default();
                if s.col_span.is_some() {
                    runs.push(Vec::new());
                    continue;
                }
                match s.h_merge.as_deref() {
                    Some("restart") => runs.push(vec![cell]),
                    Some("continue") => {
                        if let Some(last) = runs.last_mut() {
                            last.push(cell);
                        }
                    }
                    _ => runs.push(Vec::new()),
                }
            }
            self.commit_merge_run(runs, |cs, len| cs.col_span = Some(len));
        }
    }

    fn commit_merge_run(
        &mut self,
        runs: Vec<Vec<Node<'a, 'i>>>,
        set_span: impl Fn(&mut CellStyle, i64),
    ) {
        for run in runs {
            if run.len() > 1 {
                if let Some(&head) = run.first() {
                    if let Some(cs) = self.style_map_cell.get_mut(&head) {
                        set_span(cs, run.len() as i64);
                    }
                }
                for &tc in &run[1..] {
                    self.removed_cells.insert(tc);
                }
            }
        }
    }
}

/// Every table in a document, plus a flattened paragraph-style lookup
/// across all of them (including nested sub-tables) for
/// `Styles::resolve_paragraph`/`resolve_run` (not yet ported) to
/// consult.
///
/// Port of the Python `Tables` -- reading (`register`) and the two
/// style lookups only; `apply_markup` is deferred (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct Tables<'a, 'i> {
    pub tables: Vec<Table<'a, 'i>>,
    para_styles: HashMap<Node<'a, 'i>, (Option<ParagraphStyle>, Option<RunStyle>)>,
    /// Every `w:tbl` node that is a *direct* sub-table (one level, see
    /// [`Table::new`]'s docs) of some already-registered table, so a
    /// caller's own top-level `w:tbl` walk doesn't double-register it.
    sub_table_nodes: HashSet<Node<'a, 'i>>,
}

impl<'a, 'i> Tables<'a, 'i> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of the Python `Tables.register`.
    pub fn register(
        &mut self,
        tbl: Node<'a, 'i>,
        named_styles: &HashMap<String, Style>,
        ns: &DocxNamespace,
    ) {
        if self.sub_table_nodes.contains(&tbl) {
            return;
        }
        let table = Table::new(tbl, named_styles, ns, false);
        self.collect_para_styles(&table);
        for &sub in table.sub_tables.keys() {
            self.sub_table_nodes.insert(sub);
        }
        self.tables.push(table);
    }

    /// Recursively (all nesting depths, unlike [`Table::sub_tables`]'s
    /// own one-level-deep XPath scope) copies each table's resolved
    /// paragraph styles into the flat [`Tables::para_styles`] map.
    fn collect_para_styles(&mut self, table: &Table<'a, 'i>) {
        for (&p, styles) in &table.style_map_para {
            self.para_styles.insert(p, styles.clone());
        }
        for sub in table.sub_tables.values() {
            self.collect_para_styles(sub);
        }
    }

    /// Port of the Python `Tables.para_style`.
    pub fn para_style(&self, p: Node<'a, 'i>) -> Option<&ParagraphStyle> {
        self.para_styles.get(&p)?.0.as_ref()
    }

    /// Port of the Python `Tables.run_style`.
    pub fn run_style(&self, p: Node<'a, 'i>) -> Option<&RunStyle> {
        self.para_styles.get(&p)?.1.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    fn parse(tag: &str, body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(
            format!(
                r#"<{tag} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</{tag}>"#
            )
            .into_boxed_str(),
        );
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    fn row_style_of(body: &str) -> RowStyle {
        let (doc, ns) = parse("w:trPr", body);
        RowStyle::from_trpr(doc.root_element(), &ns)
    }

    fn cell_style_of(body: &str) -> CellStyle {
        let (doc, ns) = parse("w:tcPr", body);
        CellStyle::from_tcpr(doc.root_element(), &ns)
    }

    fn table_style_of(body: &str) -> TableStyle {
        let (doc, ns) = parse("w:tblPr", body);
        TableStyle::from_tblpr(doc.root_element(), &ns)
    }

    #[test]
    fn row_style_reads_height_and_binary_props() {
        let s = row_style_of(r#"<w:trHeight w:val="480" w:hRule="atLeast"/><w:cantSplit/>"#);
        assert_eq!(s.cant_split, Some(true));
        assert_eq!(s.hidden, None);
        assert_eq!(s.css().get("min-height").map(String::as_str), Some("24pt"));
    }

    #[test]
    fn row_style_hidden_and_spacing_css() {
        let s = row_style_of(r#"<w:hidden/><w:tblCellSpacing w:w="100" w:type="dxa"/>"#);
        let css = s.css();
        assert_eq!(css.get("display").map(String::as_str), Some("none"));
        assert_eq!(
            css.get("border-collapse").map(String::as_str),
            Some("separate")
        );
        assert_eq!(css.get("border-spacing").map(String::as_str), Some("5pt"));
    }

    #[test]
    fn cell_width_reads_dxa_pct_and_nil() {
        let s = cell_style_of(r#"<w:tcW w:w="2880" w:type="dxa"/>"#);
        assert_eq!(s.width.as_deref(), Some("144pt"));

        let s = cell_style_of(r#"<w:tcW w:w="2500" w:type="pct"/>"#);
        assert_eq!(s.width.as_deref(), Some("50%"));

        let s = cell_style_of(r#"<w:tcW w:w="0" w:type="nil"/>"#);
        assert_eq!(s.width.as_deref(), Some("0"));
    }

    #[test]
    fn cell_padding_defaults_left_and_right_when_unset() {
        let s = cell_style_of("");
        let css = s.css();
        assert_eq!(css.get("padding-left").map(String::as_str), Some("5.75pt"));
        assert_eq!(css.get("padding-right").map(String::as_str), Some("5.75pt"));
        assert_eq!(css.get("padding-top").map(String::as_str), Some("0.5ex"));
        assert_eq!(css.get("padding-bottom").map(String::as_str), Some("0.5ex"));
        assert_eq!(css.get("vertical-align").map(String::as_str), Some("top"));
    }

    #[test]
    fn cell_merge_and_span_are_read() {
        let s = cell_style_of(r#"<w:gridSpan w:val="3"/><w:vMerge w:val="restart"/>"#);
        assert_eq!(s.col_span, Some(3));
        assert_eq!(s.v_merge.as_deref(), Some("restart"));

        let s = cell_style_of("<w:hMerge/>");
        assert_eq!(s.h_merge.as_deref(), Some("continue"));
    }

    #[test]
    fn table_style_reads_width_bidi_and_band_sizes() {
        let s = table_style_of(
            r#"<w:tblW w:w="5000" w:type="pct"/><w:bidiVisual/><w:tblStyleColBandSize w:val="2"/>"#,
        );
        assert_eq!(s.width.as_deref(), Some("100%"));
        assert_eq!(s.bidi, Some(true));
        assert_eq!(s.col_band_size, 2);
        assert_eq!(s.row_band_size, 1);
    }

    #[test]
    fn table_style_justification_sets_auto_margins() {
        let s = table_style_of(r#"<w:jc w:val="center"/>"#);
        assert_eq!(s.margin_left.as_deref(), Some("auto"));
        assert_eq!(s.margin_right.as_deref(), Some("auto"));
    }

    #[test]
    fn table_style_look_parses_hex() {
        let s = table_style_of(r#"<w:tblLook w:val="04A0"/>"#);
        assert_eq!(s.look, 0x04A0);
    }

    #[test]
    fn table_style_borders_use_six_edges() {
        let s = table_style_of(
            r#"<w:tblBorders><w:insideH w:val="single" w:sz="8" w:space="0" w:color="000000"/></w:tblBorders>"#,
        );
        assert_eq!(s.borders.inside_h.style.as_deref(), Some("solid"));
    }

    #[test]
    fn table_style_update_only_overwrites_set_fields() {
        let mut base = TableStyle::new();
        base.width = Some("10pt".to_string());
        base.look = 5;
        let mut other = TableStyle::new();
        other.margin_left = Some("auto".to_string());
        other.look = 9;
        base.update(&other);
        assert_eq!(base.width.as_deref(), Some("10pt"), "unset in other, kept");
        assert_eq!(base.margin_left.as_deref(), Some("auto"));
        assert_eq!(base.look, 9, "look is never inherit, always overwritten");
    }

    #[test]
    fn table_style_resolve_based_on_only_fills_gaps() {
        let mut child = TableStyle::new();
        child.width = Some("10pt".to_string());
        let mut parent = TableStyle::new();
        parent.width = Some("20pt".to_string());
        parent.margin_left = Some("5pt".to_string());
        child.resolve_based_on(&parent);
        assert_eq!(
            child.width.as_deref(),
            Some("10pt"),
            "child's own value wins"
        );
        assert_eq!(
            child.margin_left.as_deref(),
            Some("5pt"),
            "gap filled from parent"
        );
    }

    #[test]
    fn table_style_pr_inside_a_named_style_extracts_overrides() {
        let xml: &'static str = Box::leak(
            r#"<w:style xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" w:type="table" w:styleId="Tbl">
                <w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr>
                <w:tblStylePr w:type="firstRow">
                    <w:tcPr><w:shd w:fill="FF0000"/></w:tcPr>
                </w:tblStylePr>
            </w:style>"#
                .to_string()
                .into_boxed_str(),
        );
        let doc = Document::parse(xml).expect("valid XML");
        let ns = DocxNamespace::default();
        let style_elem = doc.root_element();
        let tblpr = ns
            .children(style_elem, &["w:tblPr"])
            .into_iter()
            .next()
            .unwrap();
        let s = TableStyle::from_tblpr(tblpr, &ns);
        let overrides = s.overrides.expect("style parent means Some(..)");
        let first_row = overrides
            .get("firstRow")
            .expect("firstRow override present");
        let cell = first_row.cell.as_ref().expect("tcPr override present");
        assert_eq!(cell.background_color.as_deref(), Some("#FF0000"));
    }

    fn parse_tbl(body: &str) -> (Document<'static>, DocxNamespace) {
        parse("w:tbl", body)
    }

    fn simple_2x2_table() -> &'static str {
        r#"<w:tr><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr>
           <w:tr><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr>"#
    }

    #[test]
    fn table_construction_populates_cell_map_and_style_maps() {
        let (doc, ns) = parse_tbl(simple_2x2_table());
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        assert_eq!(table.cell_map.len(), 2, "two rows");
        assert_eq!(table.cell_map[0].len(), 2, "two cells per row");
        assert_eq!(table.paragraphs.len(), 4, "one paragraph per cell");
        assert_eq!(table.style_map_row.len(), 2);
        assert_eq!(table.style_map_cell.len(), 4);
        assert_eq!(table.style_map_para.len(), 4);
        assert!(table.sub_tables.is_empty());
        assert!(table.removed_cells.is_empty());
    }

    #[test]
    fn table_style_is_read_from_a_named_style_and_direct_formatting() {
        let (doc, ns) = parse_tbl(
            r#"<w:tblPr><w:tblStyle w:val="MyTable"/><w:tblW w:w="5000" w:type="pct"/></w:tblPr>
               <w:tr><w:tc><w:p/></w:tc></w:tr>"#,
        );
        let mut named_styles = HashMap::new();
        let mut style = Style::default();
        style.table_style = Some({
            let mut ts = TableStyle::new();
            ts.background_color = Some("#00ff00".to_string());
            ts
        });
        named_styles.insert("MyTable".to_string(), style);

        let table = Table::new(doc.root_element(), &named_styles, &ns, false);
        assert_eq!(
            table.table_style.background_color.as_deref(),
            Some("#00ff00"),
            "inherited from the named style"
        );
        assert_eq!(
            table.table_style.width.as_deref(),
            Some("100%"),
            "direct tblW overlays on top of the named style"
        );
    }

    #[test]
    fn get_overrides_flags_corners_and_edges_on_a_2x2_table() {
        let (doc, ns) = parse_tbl(simple_2x2_table());
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        // Every band/look bit is off by default, so `override_allowed`
        // only lets through "*Cell"/"wholeTable" names -- band overrides
        // never survive the filter under default `look`.
        let nw = table.get_overrides(0, Some(0), 2, Some(2));
        assert!(nw.contains(&"wholeTable".to_string()));
        assert!(nw.contains(&"nwCell".to_string()));
        let se = table.get_overrides(1, Some(1), 2, Some(2));
        assert!(se.contains(&"seCell".to_string()));
    }

    #[test]
    fn override_allowed_respects_the_tbl_look_bitmask() {
        let (doc, ns) = parse_tbl(
            r#"<w:tblPr><w:tblLook w:val="0020"/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        assert!(table.override_allowed("firstRow"), "0x0020 allows firstRow");
        assert!(!table.override_allowed("lastRow"), "0x0040 not set");
    }

    #[test]
    fn cell_style_inherits_padding_from_the_table_style() {
        let (doc, ns) = parse_tbl(
            r#"<w:tblPr><w:tblCellMar><w:left w:w="200" w:type="dxa"/></w:tblCellMar></w:tblPr>
               <w:tr><w:tc><w:p/></w:tc></w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        let tc = table.cell_map[0][0];
        let cs = &table.style_map_cell[&tc];
        assert_eq!(
            cs.css().get("padding-left").map(String::as_str),
            Some("10pt"),
            "cell has no cellMar of its own, falls back to the table's"
        );
    }

    #[test]
    fn cell_border_falls_back_to_the_inside_edge_then_to_hidden() {
        // A 1x2 row: the second cell's left edge is an inside edge
        // (col > 0), so with no explicit border anywhere it should
        // fall back through insideV -- which is also unset here, so it
        // stays unset (not "none"->"hidden", since that swap is only
        // for the *outer* edges).
        let (doc, ns) = parse_tbl(
            r#"<w:tr><w:tc><w:p/></w:tc><w:tc><w:tcPr><w:tcBorders><w:left w:val="none"/></w:tcBorders></w:tcPr><w:p/></w:tc></w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        let second_tc = table.cell_map[0][1];
        let cs = &table.style_map_cell[&second_tc];
        assert_eq!(
            cs.borders.left.style.as_deref(),
            Some("none"),
            "an inside edge keeps a literal none rather than becoming hidden"
        );

        // The *first* cell's left edge is an outer edge (col == 0); an
        // explicit "none" there does become "hidden".
        let (doc2, ns2) = parse_tbl(
            r#"<w:tr><w:tc><w:tcPr><w:tcBorders><w:left w:val="none"/></w:tcBorders></w:tcPr><w:p/></w:tc></w:tr>"#,
        );
        let table2 = Table::new(doc2.root_element(), &HashMap::new(), &ns2, false);
        let first_tc = table2.cell_map[0][0];
        let cs2 = &table2.style_map_cell[&first_tc];
        assert_eq!(cs2.borders.left.style.as_deref(), Some("hidden"));
    }

    #[test]
    fn vertical_merge_sets_row_span_and_marks_continuations_removed() {
        let (doc, ns) = parse_tbl(
            r#"<w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p/></w:tc></w:tr>
               <w:tr><w:tc><w:tcPr><w:vMerge w:val="continue"/></w:tcPr><w:p/></w:tc></w:tr>
               <w:tr><w:tc><w:tcPr><w:vMerge w:val="continue"/></w:tcPr><w:p/></w:tc></w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        let head = table.cell_map[0][0];
        let cont1 = table.cell_map[1][0];
        let cont2 = table.cell_map[2][0];
        assert_eq!(table.style_map_cell[&head].row_span, Some(3));
        assert!(table.removed_cells.contains(&cont1));
        assert!(table.removed_cells.contains(&cont2));
        assert!(!table.removed_cells.contains(&head));
    }

    #[test]
    fn horizontal_merge_sets_col_span_and_marks_continuations_removed() {
        let (doc, ns) = parse_tbl(
            r#"<w:tr>
                <w:tc><w:tcPr><w:hMerge w:val="restart"/></w:tcPr><w:p/></w:tc>
                <w:tc><w:tcPr><w:hMerge w:val="continue"/></w:tcPr><w:p/></w:tc>
               </w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        let head = table.cell_map[0][0];
        let cont = table.cell_map[0][1];
        assert_eq!(table.style_map_cell[&head].col_span, Some(2));
        assert!(table.removed_cells.contains(&cont));
    }

    #[test]
    fn a_cell_with_its_own_grid_span_is_never_treated_as_an_hmerge_continuation() {
        let (doc, ns) = parse_tbl(
            r#"<w:tr>
                <w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p/></w:tc>
                <w:tc><w:tcPr><w:hMerge w:val="continue"/></w:tcPr><w:p/></w:tc>
               </w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        let second = table.cell_map[0][1];
        // The gridSpan cell breaks the run, so the lone "continue" cell
        // starts (and immediately ends) its own single-element run,
        // which is never long enough to be marked removed.
        assert!(!table.removed_cells.contains(&second));
    }

    #[test]
    fn nested_table_is_ported_as_a_one_level_deep_sub_table() {
        let (doc, ns) = parse_tbl(
            r#"<w:tr><w:tc>
                <w:p/>
                <w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
               </w:tc></w:tr>"#,
        );
        let table = Table::new(doc.root_element(), &HashMap::new(), &ns, false);
        assert_eq!(table.sub_tables.len(), 1);
        // The outer table's own paragraph collection only looks at
        // `./w:tc/w:p` (direct children), so the nested tbl's paragraph
        // is not double-counted here.
        assert_eq!(table.paragraphs.len(), 1);
    }

    #[test]
    fn tables_register_flattens_para_styles_across_sub_tables_and_skips_double_registration() {
        let (doc, ns) = parse_tbl(
            r#"<w:tr><w:tc>
                <w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>
               </w:tc></w:tr>"#,
        );
        let mut tables = Tables::new();
        tables.register(doc.root_element(), &HashMap::new(), &ns);
        assert_eq!(tables.tables.len(), 1);

        let outer = &tables.tables[0];
        let (&sub_tbl_node, sub_table) = outer.sub_tables.iter().next().unwrap();
        let inner_p = sub_table.paragraphs[0];
        assert!(
            tables.para_styles.contains_key(&inner_p),
            "sub-table paragraphs are collected recursively into the flat lookup"
        );

        // Registering the nested tbl node directly (as a caller's own
        // top-level `w:tbl` walk would also encounter it) is a no-op.
        tables.register(sub_tbl_node, &HashMap::new(), &ns);
        assert_eq!(
            tables.tables.len(),
            1,
            "already a sub-table, not re-registered"
        );
    }
}
