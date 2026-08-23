//! Table, row and cell formatting: reading `w:tblPr`/`w:trPr`/`w:tcPr`
//! into property sets and turning them into CSS.
//!
//! Partial port of `old_src/src/calibre/ebooks/docx/tables.py` — the
//! `Style`/`RowStyle`/`CellStyle`/`TableStyle` property models only.
//! `Table`/`Tables` (which build the HTML `<table>` markup and, in
//! `handle_merged_cells`, remove merged-away `w:tc` elements from the
//! *source* document tree) are deferred to the same follow-up as
//! `to_html.rs`'s real port: both need a mutable tree, and the source
//! tree is currently read-only `roxmltree` just like the HTML side
//! was before [`crate::dom`]. See issue #130.
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

use indexmap::IndexMap;
use roxmltree::Node;

use super::block_styles::{
    border_to_css, format_g3, pt, read_border, Borders, Css, Edge, ParagraphStyle,
};
use super::char_styles::RunStyle;
use super::names::DocxNamespace;

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
}
