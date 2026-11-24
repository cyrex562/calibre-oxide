//! Tables (`docx/writer/tables.py`) -- **fully ported**. The
//! border/width foundation, [`SpannedCell`]/[`Cell`]/[`Row`]/[`Table`]
//! themselves, their border-conflict-resolution algorithms, the
//! stateful HTML-walk mutation methods, AND every `.serialize` method
//! are all ported (`Blocks`' OWN half of the HTML-walk integration --
//! its `tables` stack of currently-open tables, `finish_tag`'s
//! table-closing branch, and `Block`'s `container` field -- lives in
//! `from_html.rs`, also ported).
//!
//! Ported: [`Border`], [`border_style_weight`], [`as_percent`],
//! [`convert_width`], and [`read_css_block_borders`] (a thin
//! per-edge-`Border` repackaging of
//! [`super::styles::read_css_block_borders`], already real). Python's
//! version does this repackaging by mutating a throwaway `Dummy()`
//! object with `setattr`/`getattr` (`styles.py`'s
//! `read_css_block_borders` was written to mutate `self` in place,
//! since its only other caller, `BlockStyle`, IS a real `self`) --
//! this port's `styles::read_css_block_borders` already returns real
//! structs instead of mutating anything, so no `Dummy` stand-in is
//! needed at all.
//!
//! [`SpannedCell`], [`Cell`], [`Row`], and [`Table`] are held in a
//! [`Tables`] arena (see its own docs for the arena-shape decision
//! this needed -- the real, previously undecided design question this
//! module was once blocked on). Ported with them:
//! [`Tables::neighbor`]/`::applicable_borders`/`::resolve_border`/
//! `::resolve_cell_borders` (CSS table border-conflict resolution),
//! [`Tables::expand_spanned_cells`] (inserting merge-continuation
//! placeholders for `rowspan`/`colspan`'d cells), the stateful
//! mutation half ([`Tables::table_start_new_row`]/`::table_start_new_cell`/
//! `::table_finish_tag`/`::table_add_block`/`::table_add_table`/
//! `::row_add_block`/`::row_add_table`/`::cell_add_block`/
//! `::cell_add_table`, plus their `row_*`/`Row`-level counterparts),
//! and now [`SpannedCell::serialize`]/[`Tables::serialize_cell`]/
//! `::serialize_row`/`::serialize_table` (the actual `<w:tc>`/`<w:tr>`/
//! `<w:tbl>` OOXML). [`Cell::items`] (mixing `Block`s and nested
//! `Table`s, port of `Cell.items`) uses the
//! [`super::from_html::ItemId`] enum, shared with `Blocks`' own
//! top-level `items` list (also real, `from_html.rs`).
//!
//! **The one real design point in the serialize methods**: rendering a
//! `Block`'s own content needs `StylesManager`/`LinksManager` (both
//! owned by `Blocks`, in `from_html.rs`, per that module's own
//! "explicit parameter, not stored field" pattern) -- this arena
//! can't resolve that on its own, and importing those types here would
//! invert this effort's established module-dependency direction
//! (`from_html.rs` already depends on `tables.rs`, not the reverse).
//! [`Tables::serialize_cell`]/`::serialize_row`/`::serialize_table`
//! instead take a `serialize_block: &mut dyn FnMut(&mut Element,
//! BlockId)` callback for exactly that one case -- a nested
//! [`ItemId::Table`] needs no such callback, since it recurses
//! straight back into this arena's own `serialize_table`. `Blocks`'
//! own `.serialize` (not yet ported) is what will build the real
//! closure and pass it in.

use crate::dom::{Dom, NodeId};
use crate::oeb::polish::cascade::ResolvedStyles;
use crate::oeb::polish::style::{Profile, Style};

use super::from_html::{BlockId, ItemId};
use super::styles::{read_css_block_borders as read_block_borders, BORDER_EDGES};
use super::utils::convert_color;
use super::xml::Element;

/// Port of `Border`. `level` is `Cell`/`Row`/`Table`'s `BLEVEL` class
/// attribute (2/1/0 respectively) -- how specific the border's source
/// element was, used by `resolve_border`'s (not yet ported) CSS
/// table-border-conflict-resolution weighing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Border {
    pub css_style: String,
    pub style: String,
    pub width: i64,
    pub color: Option<String>,
    pub level: u8,
}

/// Port of `border_style_weight`: how strongly a `border-style`
/// keyword wins CSS's table border-conflict resolution when widths
/// are tied. Unlisted keywords (`none`, `hidden`, or anything
/// unrecognized) weigh `0`, matching Python's `.get(x, 0)`.
pub fn border_style_weight(style: &str) -> i32 {
    const ORDER: [&str; 8] = [
        "double", "solid", "dashed", "dotted", "ridge", "outset", "groove", "inset",
    ];
    match ORDER.iter().position(|&s| s == style) {
        Some(i) => 100 - i as i32,
        None => 0,
    }
}

/// Port of `as_percent`.
pub fn as_percent(x: &str) -> Option<f64> {
    if x.ends_with('%') {
        x.trim_end_matches('%').parse::<f64>().ok()
    } else {
        None
    }
}

/// Port of `convert_width`: a `<w:tblW>`/`<w:tcW>` `(w:type, w:w)`
/// pair. `tag_style.get("width")` (Python's `_get`, the raw specified
/// value) decides *which* branch -- `auto`, a `%`, or a length -- but
/// the length branch reads `Style::width()` (Python's `tag_style['width']`,
/// the dedicated, fully-resolved property -- `__getitem__` dispatches
/// to it since `width` has one, per this port's domName-dispatch rule),
/// not the raw specified string. Python's `try/except` around the
/// length branch is unreachable here: `Style::width()` already has
/// its own internal fallback and never raises.
pub fn convert_width(tag_style: Option<&Style>) -> (&'static str, i64) {
    let Some(style) = tag_style else {
        return ("auto", 0);
    };
    let w = style.get("width");
    if w == "auto" {
        return ("auto", 0);
    }
    if let Some(wp) = as_percent(&w) {
        return ("pct", (wp * 50.0) as i64);
    }
    ("dxa", (style.width() * 20.0) as i64)
}

/// One edge's `(border, padding)` -- the shape [`read_css_block_borders`]
/// hands to `Cell`/`Row`/`Table`'s (not yet ported) `border_{edge}`/
/// `padding_{edge}` attributes.
#[derive(Debug)]
pub struct EdgeBorders {
    pub left: Border,
    pub top: Border,
    pub right: Border,
    pub bottom: Border,
    pub padding_left: i64,
    pub padding_top: i64,
    pub padding_right: i64,
    pub padding_bottom: i64,
}

impl EdgeBorders {
    pub fn border(&self, edge: &str) -> &Border {
        match edge {
            "left" => &self.left,
            "top" => &self.top,
            "right" => &self.right,
            "bottom" => &self.bottom,
            _ => panic!("not a border edge: {edge}"),
        }
    }

    pub fn padding(&self, edge: &str) -> i64 {
        match edge {
            "left" => self.padding_left,
            "top" => self.padding_top,
            "right" => self.padding_right,
            "bottom" => self.padding_bottom,
            _ => panic!("not a border edge: {edge}"),
        }
    }
}

/// Port of `tables.py`'s own `read_css_block_borders(self, css)` --
/// repackages [`super::styles::read_css_block_borders`]'s output into
/// one [`Border`] per edge, tagged with `blevel` (`Cell`/`Row`/
/// `Table`'s `BLEVEL`). Always passes `store_css_style=true`, matching
/// `tables.py`'s own call -- it's the one real caller that needs the
/// raw CSS keyword (`Border.css_style`) alongside the resolved OOXML
/// one, to tell "explicitly `border-style: hidden`" apart from
/// "nothing declared" during border-conflict resolution (not ported
/// yet).
pub fn read_css_block_borders(css: Option<&Style>, blevel: u8) -> EdgeBorders {
    let (borders, css_styles) = read_block_borders(css, true);
    let css_styles = css_styles.expect("store_css_style=true always returns Some");
    let border = |edge: &str| -> Border {
        let (css_style, style, width, color) = match edge {
            "left" => (
                &css_styles.border_left_css_style,
                &borders.border_left_style,
                borders.border_left_width,
                &borders.border_left_color,
            ),
            "top" => (
                &css_styles.border_top_css_style,
                &borders.border_top_style,
                borders.border_top_width,
                &borders.border_top_color,
            ),
            "right" => (
                &css_styles.border_right_css_style,
                &borders.border_right_style,
                borders.border_right_width,
                &borders.border_right_color,
            ),
            "bottom" => (
                &css_styles.border_bottom_css_style,
                &borders.border_bottom_style,
                borders.border_bottom_width,
                &borders.border_bottom_color,
            ),
            _ => unreachable!(),
        };
        Border {
            css_style: css_style.clone(),
            style: style.clone(),
            width,
            color: color.clone(),
            level: blevel,
        }
    };
    debug_assert_eq!(BORDER_EDGES, ["left", "top", "right", "bottom"]);
    EdgeBorders {
        left: border("left"),
        top: border("top"),
        right: border("right"),
        bottom: border("bottom"),
        padding_left: borders.padding_left,
        padding_top: borders.padding_top,
        padding_right: borders.padding_right,
        padding_bottom: borders.padding_bottom,
    }
}

/// Port of `Cell`/`Row`/`Table.background_color`'s shared one-liner:
/// `None if tag_style is None else convert_color(tag_style.backgroundColor)`.
pub fn table_background_color(tag_style: Option<&Style>) -> Option<String> {
    convert_color(tag_style?.background_color().as_deref())
}

/// Handle into a [`Tables`]' `Table` arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TableId(pub usize);

/// Handle into a [`Tables`]' `Row` arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RowId(pub usize);

/// Handle into a [`Tables`]' [`CellSlot`] arena -- shared by real
/// [`Cell`]s and [`SpannedCell`] placeholders, see [`CellSlot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellId(pub usize);

/// Port of `SpannedCell`: a merge-continuation placeholder Word needs
/// in every physical grid position a `rowspan`/`colspan`'d [`Cell`]
/// covers besides its own.
#[derive(Debug, Clone, Copy)]
pub struct SpannedCell {
    /// The real [`Cell`] this placeholder continues the merge of.
    pub spanning_cell: CellId,
    pub horizontal: bool,
}

impl SpannedCell {
    /// Port of `SpannedCell.serialize`: a bare `<w:tc>` with a
    /// `w:hMerge`/`w:vMerge` `val="continue"` marker and an empty
    /// paragraph -- Word requires every grid position to have a real
    /// `<w:tc>`, even the ones a merge covers.
    pub fn serialize(&self, parent: &mut Element) {
        let tc = parent.append(Element::new("w:tc"));
        let tc_pr = tc.append(Element::new("w:tcPr"));
        let merge_tag = if self.horizontal {
            "w:hMerge"
        } else {
            "w:vMerge"
        };
        tc_pr.append(Element::new(merge_tag).attr("w:val", "continue"));
        tc.append(Element::new("w:p"));
    }
}

/// One slot in a [`Row`]'s cell list: either a real [`Cell`] or a
/// [`SpannedCell`] merge placeholder. Python's `row.cells` mixes
/// `Cell`/`SpannedCell` objects, told apart with `isinstance` --
/// this is the real enum standing in for that duck typing, letting
/// [`Row::cells`] stay one homogeneous `Vec<CellId>`.
#[derive(Debug)]
pub enum CellSlot {
    Cell(Cell),
    Spanned(SpannedCell),
}

/// Port of `Cell`. `BLEVEL = 2` (passed to
/// [`read_css_block_borders`] as the most-specific border source).
///
/// **Not ported yet, deliberately**: `serialize` -- needs a real
/// `w:tc` builder against this cell's now-real `items` content, left
/// for the `Blocks` HTML-walk-integration PR that will actually
/// populate `items` during a real conversion.
#[derive(Debug)]
pub struct Cell {
    pub row: RowId,
    pub table: TableId,
    pub html_tag: NodeId,
    pub row_span: u32,
    pub col_span: u32,
    /// `None` when `vertical-align` wasn't `top`/`bottom`/`middle` --
    /// Python's `{'top':.., 'bottom':.., 'middle':..}.get(...)`
    /// silently drops anything else, including a genuinely absent
    /// declaration. Only defaults to `Some("center")` when there is
    /// no `tag_style` at all (a table with no CSS at all).
    pub valign: Option<&'static str>,
    pub width: (&'static str, i64),
    pub background_color: Option<String>,
    pub borders: EdgeBorders,
    /// Set by [`Tables::resolve_cell_borders`] once the whole table
    /// is built -- `None` until then, matching Python's `self.borders`
    /// only existing after `Table.finish_tag`'s `resolve_borders()`
    /// pass runs (referencing it earlier would be an `AttributeError`
    /// in Python; here it's a `None` a caller has to handle instead).
    pub resolved_borders: Option<ResolvedBorders>,
    /// Port of `Cell.items`: this cell's own content, mixing `Block`s
    /// and nested `Table`s the same way
    /// [`super::from_html::Blocks`]'s own top-level items list does --
    /// see [`super::from_html::ItemId`].
    pub items: Vec<ItemId>,
}

impl Cell {
    /// Port of `Cell.__init__`.
    pub fn new(
        row: RowId,
        table: TableId,
        html_tag: NodeId,
        dom: &Dom,
        tag_style: Option<&Style>,
    ) -> Cell {
        let attrs = &dom.node(html_tag).attrs;
        let row_span = span_attr(attrs, "rowspan");
        let col_span = span_attr(attrs, "colspan");
        let valign = match tag_style {
            None => Some("center"),
            Some(s) => match s.get("vertical-align").as_str() {
                "top" => Some("top"),
                "bottom" => Some("bottom"),
                "middle" => Some("center"),
                _ => None,
            },
        };
        Cell {
            row,
            table,
            html_tag,
            row_span,
            col_span,
            valign,
            width: convert_width(tag_style),
            background_color: table_background_color(tag_style),
            borders: read_css_block_borders(tag_style, 2),
            resolved_borders: None,
            items: Vec::new(),
        }
    }
}

/// Parses an HTML `rowspan`/`colspan` attribute: `max(0, value)` when
/// present and numeric, `1` (Python's `except Exception: ... = 1`
/// fallback) when absent or unparseable -- deliberately not
/// [`super::utils::int_or_zero`], whose failure default is `0`, not
/// `1`.
fn span_attr(attrs: &indexmap::IndexMap<String, String>, name: &str) -> u32 {
    match attrs.get(name).and_then(|v| v.trim().parse::<i64>().ok()) {
        Some(n) => n.max(0) as u32,
        None => 1,
    }
}

/// Port of `Row`. `BLEVEL = 1`.
///
/// **Not ported yet, deliberately**: `serialize` -- see [`Cell`]'s
/// docs.
#[derive(Debug)]
pub struct Row {
    pub table: TableId,
    pub html_tag: NodeId,
    pub cells: Vec<CellId>,
    pub background_color: Option<String>,
    pub borders: EdgeBorders,
    /// Port of `Row.current_cell`: the cell currently being built,
    /// not yet moved into [`Self::cells`] (that happens on
    /// [`Tables::table_finish_tag`]'s matching closing tag).
    pub current_cell: Option<CellId>,
    /// Port of `Row.orig_tag_style`. Python holds a live `Style`
    /// object (cheap there); this stores just the `Style::node` it
    /// was built from and reconstructs a real `Style` on demand
    /// (`Tables::row_add_block`/`row_add_table`'s auto-create-a-cell
    /// fallback) -- the same "stored `NodeId`, rebuilt `Style`"
    /// pattern this effort uses throughout (e.g. `Block::html_block`).
    /// `None` when the row was itself auto-created with no real style
    /// (Python's `Row(self, html_tag, None)`).
    pub orig_tag_style_node: Option<NodeId>,
}

impl Row {
    pub fn new(table: TableId, html_tag: NodeId, tag_style: Option<&Style>) -> Row {
        Row {
            table,
            html_tag,
            cells: Vec::new(),
            background_color: table_background_color(tag_style),
            borders: read_css_block_borders(tag_style, 1),
            current_cell: None,
            orig_tag_style_node: tag_style.map(|s| s.node),
        }
    }

    pub fn first_cell(&self) -> Option<CellId> {
        self.cells.first().copied()
    }

    pub fn last_cell(&self) -> Option<CellId> {
        self.cells.last().copied()
    }
}

/// Port of `Table`. `BLEVEL = 0`.
///
/// **Not ported yet, deliberately**: `serialize` -- see [`Cell`]'s
/// docs.
#[derive(Debug)]
pub struct Table {
    pub html_tag: NodeId,
    pub rows: Vec<RowId>,
    pub width: (&'static str, i64),
    pub background_color: Option<String>,
    pub jc: Option<&'static str>,
    pub float: Option<String>,
    pub margin_left: Option<f64>,
    pub margin_right: Option<f64>,
    pub margin_top: Option<f64>,
    pub margin_bottom: Option<f64>,
    pub borders: EdgeBorders,
    /// Port of `Table.current_row`: the row currently being built,
    /// not yet moved into [`Self::rows`].
    pub current_row: Option<RowId>,
    /// Port of `Table.orig_tag_style` -- see [`Row::orig_tag_style_node`]'s
    /// docs for why this stores a `NodeId`, not a live `Style`.
    pub orig_tag_style_node: Option<NodeId>,
}

impl Table {
    /// Port of `Table.__init__`. The `ml`/`mr` auto-margin check
    /// reads the raw specified value (`Style::get`, Python's `._get`/
    /// `.get()` -- same as [`convert_width`]'s own auto check, not a
    /// new domName-dispatch decision), but the four `margin_*` fields
    /// use the dedicated, already-unit-converted `Style::margin_*`
    /// accessors (Python's `tag_style['margin-' + edge]`, which DOES
    /// dispatch to a real `@property`).
    pub fn new(html_tag: NodeId, tag_style: Option<&Style>) -> Table {
        let mut jc = None;
        let mut float = None;
        let mut margin_left = None;
        let mut margin_right = None;
        let mut margin_top = None;
        let mut margin_bottom = None;
        if let Some(style) = tag_style {
            let ml = style.get("margin-left");
            let mr = style.get("margin-right");
            if ml == "auto" {
                jc = Some(if mr == "auto" { "center" } else { "right" });
            }
            float = Some(style.get("float"));
            margin_left = Some(style.margin_left());
            margin_right = Some(style.margin_right());
            margin_top = Some(style.margin_top());
            margin_bottom = Some(style.margin_bottom());
        }
        Table {
            html_tag,
            rows: Vec::new(),
            width: convert_width(tag_style),
            background_color: table_background_color(tag_style),
            jc,
            float,
            margin_left,
            margin_right,
            margin_top,
            margin_bottom,
            borders: read_css_block_borders(tag_style, 0),
            current_row: None,
            orig_tag_style_node: tag_style.map(|s| s.node),
        }
    }

    pub fn first_row(&self) -> Option<RowId> {
        self.rows.first().copied()
    }

    pub fn last_row(&self) -> Option<RowId> {
        self.rows.last().copied()
    }
}

/// A [`Cell`]'s final, border-conflict-resolved edges -- port of
/// `Cell.borders` (the `dict` [`Tables::resolve_cell_borders`]
/// computes), stored separately from [`Cell::borders`] (that cell's
/// own *raw* CSS borders, `BLEVEL = 2`, used as one input to conflict
/// resolution alongside its row/table's own raw borders and its
/// neighbor's).
#[derive(Debug, Clone, Default)]
pub struct ResolvedBorders {
    pub left: Option<Border>,
    pub top: Option<Border>,
    pub right: Option<Border>,
    pub bottom: Option<Border>,
}

impl ResolvedBorders {
    fn set(&mut self, edge: &str, value: Option<Border>) {
        match edge {
            "left" => self.left = value,
            "top" => self.top = value,
            "right" => self.right = value,
            "bottom" => self.bottom = value,
            _ => panic!("not a border edge: {edge}"),
        }
    }
}

fn opposite_edge(edge: &str) -> &'static str {
    match edge {
        "left" => "right",
        "right" => "left",
        "top" => "bottom",
        "bottom" => "top",
        _ => panic!("not a border edge: {edge}"),
    }
}

/// The `Cell`/`Row`/`Table` arena -- port of the storage
/// `docx/writer/from_html.py`'s `Blocks` holds directly
/// (`self.tables`) plus what `Table`/`Row`/`Cell` reference in both
/// directions (`Cell.row`, `Cell.table`, `Row.cells`, `Table.rows`,
/// `Cell.neighbor` climbing across rows). Python holds/compares these
/// by object identity; this is the same arena-of-ids treatment
/// `Blocks`/`StylesManager` already established (`BlockId`-style
/// handles, not direct references), applied to a genuinely
/// mutually-referencing, potentially-nested graph rather than a flat
/// list.
///
/// **Not ported yet, deliberately**: the stateful HTML-walk
/// integration (`start_new_row`/`start_new_cell`/`finish_tag`'s table
/// branch, `Blocks`' own `tables` stack of currently-open tables, and
/// the `Block`-or-`Table` item enum `Blocks.items`/`Cell.items` both
/// need) and `.serialize`. This type only owns the arena and the
/// pure, already-testable algorithms (`neighbor`/`applicable_borders`/
/// `resolve_border`/`resolve_cell_borders`/`expand_spanned_cells`);
/// [`Self::add_table`]/[`Self::add_row`]/[`Self::add_cell`] are low-level
/// arena-append primitives for building a `Tables` directly (used by
/// this module's own tests), not a port of Python's stateful
/// `start_new_*` methods, which track "the current open table/row/
/// cell" during the HTML walk -- that state machine is part of the
/// deferred integration work.
#[derive(Debug, Default)]
pub struct Tables {
    tables: Vec<Table>,
    rows: Vec<Row>,
    cells: Vec<CellSlot>,
}

impl Tables {
    pub fn new() -> Tables {
        Tables::default()
    }

    pub fn add_table(&mut self, html_tag: NodeId, tag_style: Option<&Style>) -> TableId {
        self.tables.push(Table::new(html_tag, tag_style));
        TableId(self.tables.len() - 1)
    }

    pub fn add_row(
        &mut self,
        table: TableId,
        html_tag: NodeId,
        tag_style: Option<&Style>,
    ) -> RowId {
        let id = RowId(self.rows.len());
        self.rows.push(Row::new(table, html_tag, tag_style));
        self.tables[table.0].rows.push(id);
        id
    }

    pub fn add_cell(
        &mut self,
        row: RowId,
        html_tag: NodeId,
        dom: &Dom,
        tag_style: Option<&Style>,
    ) -> CellId {
        let table = self.rows[row.0].table;
        let id = CellId(self.cells.len());
        self.cells.push(CellSlot::Cell(Cell::new(
            row, table, html_tag, dom, tag_style,
        )));
        self.rows[row.0].cells.push(id);
        id
    }

    pub fn table(&self, id: TableId) -> &Table {
        &self.tables[id.0]
    }

    pub fn row(&self, id: RowId) -> &Row {
        &self.rows[id.0]
    }

    pub fn cell_slot(&self, id: CellId) -> &CellSlot {
        &self.cells[id.0]
    }

    /// The real [`Cell`] a slot resolves to, panicking if `id` names
    /// a [`CellSlot::Spanned`] -- most callers here only ever hold
    /// ids for real cells (a [`SpannedCell`] never gets its own
    /// externally-visible `CellId` handed out except via
    /// [`Row::cells`]/internal bookkeeping).
    pub fn cell(&self, id: CellId) -> &Cell {
        match &self.cells[id.0] {
            CellSlot::Cell(c) => c,
            CellSlot::Spanned(_) => panic!("CellId {} names a SpannedCell, not a Cell", id.0),
        }
    }

    /// Mutable counterpart of [`Self::cell`].
    pub fn cell_mut(&mut self, id: CellId) -> &mut Cell {
        match &mut self.cells[id.0] {
            CellSlot::Cell(c) => c,
            CellSlot::Spanned(_) => panic!("CellId {} names a SpannedCell, not a Cell", id.0),
        }
    }

    fn real_cell_id(&self, id: CellId) -> CellId {
        match &self.cells[id.0] {
            CellSlot::Cell(_) => id,
            CellSlot::Spanned(s) => s.spanning_cell,
        }
    }

    /// Port of `Cell.neighbor`: the adjacent cell across `edge`, or
    /// `None` at a grid boundary. Always resolves through
    /// [`SpannedCell::spanning_cell`] to a real [`Cell`] -- Python's
    /// `getattr(ans, 'spanning_cell', ans)`.
    pub fn neighbor(&self, cell: CellId, edge: &str) -> Option<CellId> {
        let c = self.cell(cell);
        let row = &self.rows[c.row.0];
        let idx = row.cells.iter().position(|&id| id == cell)?;
        let raw = match edge {
            "left" => idx.checked_sub(1).map(|i| row.cells[i]),
            "right" => row.cells.get(idx + 1).copied(),
            "top" | "bottom" => {
                let table = &self.tables[c.table.0];
                let ridx = table.rows.iter().position(|&id| id == c.row)?;
                let nridx = if edge == "top" {
                    ridx.checked_sub(1)
                } else {
                    Some(ridx + 1)
                }?;
                let nrow_id = *table.rows.get(nridx)?;
                self.rows[nrow_id.0].cells.get(idx).copied()
            }
            _ => panic!("not a border edge: {edge}"),
        };
        raw.map(|id| self.real_cell_id(id))
    }

    /// Port of `Cell.applicable_borders`/`SpannedCell.applicable_borders`
    /// (the latter just delegates to the real cell first).
    pub fn applicable_borders(&self, cell: CellId, edge: &str) -> Vec<Border> {
        let real = self.real_cell_id(cell);
        let c = self.cell(real);
        let row = &self.rows[c.row.0];
        let table = &self.tables[c.table.0];
        let mut out = Vec::new();
        match edge {
            "left" => {
                if row.first_cell() == Some(real) {
                    out.push(table.borders.border("left").clone());
                    out.push(row.borders.border("left").clone());
                }
                out.push(c.borders.border("left").clone());
            }
            "right" => {
                if row.last_cell() == Some(real) {
                    out.push(table.borders.border("right").clone());
                    out.push(row.borders.border("right").clone());
                }
                out.push(c.borders.border("right").clone());
            }
            "top" => {
                if table.first_row() == Some(c.row) {
                    out.push(table.borders.border("top").clone());
                }
                out.push(c.borders.border("top").clone());
                out.push(row.borders.border("top").clone());
            }
            "bottom" => {
                if table.last_row() == Some(c.row) {
                    out.push(table.borders.border("bottom").clone());
                }
                out.push(c.borders.border("bottom").clone());
                out.push(row.borders.border("bottom").clone());
            }
            _ => panic!("not a border edge: {edge}"),
        }
        out
    }

    /// Port of `Cell.resolve_border`: the single winning [`Border`]
    /// for `edge` after CSS table border-conflict resolution across
    /// this cell, its row/table, and its neighbor across that edge --
    /// or `None` if any applicable border is explicitly `hidden`.
    ///
    /// Python breaks ties among equally-weighted borders via a
    /// hash-randomized `set`'s iteration order (`color` isn't part of
    /// the weight tuple, so two borders differing only in color CAN
    /// tie) -- genuinely nondeterministic there too. This collects
    /// candidates in a fixed, documented order (self's own applicable
    /// borders, then the neighbor's) and keeps the LAST-seen among
    /// ties (`Iterator::max_by_key`'s documented tie behavior),
    /// matching Python's `sorted(borders, key=weight)[-1]` for a
    /// stable sort -- a disclosed, deterministic stand-in for
    /// behavior Python itself doesn't guarantee.
    pub fn resolve_border(&self, cell: CellId, edge: &str) -> Option<Border> {
        let mut borders = self.applicable_borders(cell, edge);
        if let Some(neighbor) = self.neighbor(cell, edge) {
            borders.extend(self.applicable_borders(neighbor, opposite_edge(edge)));
        }
        if borders.iter().any(|b| b.css_style == "hidden") {
            return None;
        }
        let weight = |b: &Border| -> (i32, i64, i32, u8) {
            (
                if b.css_style == "none" { 0 } else { 1 },
                b.width,
                border_style_weight(&b.css_style),
                b.level,
            )
        };
        borders.iter().max_by_key(|b| weight(b)).cloned()
    }

    /// Port of `Cell.resolve_borders`: computes and stores
    /// [`Cell::resolved_borders`] for all four edges. Call once per
    /// real cell, after [`Self::expand_spanned_cells`] has finished
    /// building the whole table (border resolution reads neighbors,
    /// which only exist once the grid is complete).
    pub fn resolve_cell_borders(&mut self, cell: CellId) {
        let mut resolved = ResolvedBorders::default();
        for edge in BORDER_EDGES {
            resolved.set(edge, self.resolve_border(cell, edge));
        }
        match &mut self.cells[cell.0] {
            CellSlot::Cell(c) => c.resolved_borders = Some(resolved),
            CellSlot::Spanned(_) => panic!("resolve_cell_borders called on a SpannedCell"),
        }
    }

    /// Port of `Table.expand_spanned_cells`: inserts [`SpannedCell`]
    /// placeholders for every grid position a `rowspan`/`colspan`'d
    /// [`Cell`] covers besides its own, so every row ends up with the
    /// same number of physical cells. Call once, when a table's
    /// closing tag is seen (Python calls it from `finish_tag`, not
    /// ported here -- see [`Table`]'s docs).
    pub fn expand_spanned_cells(&mut self, table: TableId) {
        // Expand horizontally: walk each row's *current* cells by
        // position (not Python's `tuple(row.cells)` snapshot-then-
        // reindex-by-identity dance -- a plain index walk is
        // equivalent and doesn't need Cell to be hashable/comparable).
        let row_ids = self.tables[table.0].rows.clone();
        for &row_id in &row_ids {
            let mut idx = 0;
            while idx < self.rows[row_id.0].cells.len() {
                let cell_id = self.rows[row_id.0].cells[idx];
                let col_span = match &self.cells[cell_id.0] {
                    CellSlot::Cell(c) => c.col_span,
                    CellSlot::Spanned(_) => 1,
                };
                if col_span > 1 {
                    let is_last = idx + 1 == self.rows[row_id.0].cells.len();
                    let next_is_spanned = !is_last
                        && matches!(
                            self.cells[self.rows[row_id.0].cells[idx + 1].0],
                            CellSlot::Spanned(_)
                        );
                    if is_last || !next_is_spanned {
                        for _ in 1..col_span {
                            let sc_id = CellId(self.cells.len());
                            self.cells.push(CellSlot::Spanned(SpannedCell {
                                spanning_cell: cell_id,
                                horizontal: true,
                            }));
                            idx += 1;
                            self.rows[row_id.0].cells.insert(idx, sc_id);
                        }
                    }
                }
                idx += 1;
            }
        }

        // Expand vertically.
        for r in 0..row_ids.len() {
            let row_id = row_ids[r];
            let len = self.rows[row_id.0].cells.len();
            for idx in 0..len {
                let cell_id = self.rows[row_id.0].cells[idx];
                let row_span = match &self.cells[cell_id.0] {
                    CellSlot::Cell(c) => c.row_span,
                    CellSlot::Spanned(_) => continue,
                };
                if row_span <= 1 {
                    continue;
                }
                for &nrow_id in &row_ids[r + 1..] {
                    let sc_id = CellId(self.cells.len());
                    self.cells.push(CellSlot::Spanned(SpannedCell {
                        spanning_cell: cell_id,
                        horizontal: false,
                    }));
                    let nlen = self.rows[nrow_id.0].cells.len();
                    if idx >= nlen {
                        let last = self.rows[nrow_id.0].cells.last().copied();
                        if let Some(last) = last {
                            for _ in 0..(idx - nlen) {
                                let filler_id = CellId(self.cells.len());
                                self.cells.push(CellSlot::Spanned(SpannedCell {
                                    spanning_cell: last,
                                    horizontal: true,
                                }));
                                self.rows[nrow_id.0].cells.push(filler_id);
                            }
                        }
                        self.rows[nrow_id.0].cells.push(sc_id);
                    } else if matches!(
                        self.cells[self.rows[nrow_id.0].cells[idx].0],
                        CellSlot::Spanned(_)
                    ) {
                        // Conflict between rowspan and colspan.
                        break;
                    } else {
                        self.rows[nrow_id.0].cells.insert(idx, sc_id);
                    }
                }
            }
        }
    }

    /// Port of `Row.start_new_cell`.
    pub fn row_start_new_cell(
        &mut self,
        row: RowId,
        html_tag: NodeId,
        dom: &Dom,
        tag_style: Option<&Style>,
    ) -> CellId {
        let table = self.rows[row.0].table;
        let id = CellId(self.cells.len());
        self.cells.push(CellSlot::Cell(Cell::new(
            row, table, html_tag, dom, tag_style,
        )));
        self.rows[row.0].current_cell = Some(id);
        id
    }

    /// Port of `Row.finish_tag`.
    fn row_finish_tag(&mut self, row: RowId, html_tag: NodeId) {
        if let Some(cell_id) = self.rows[row.0].current_cell {
            if self.cell(cell_id).html_tag == html_tag {
                self.rows[row.0].cells.push(cell_id);
                self.rows[row.0].current_cell = None;
            }
        }
    }

    /// Port of `Cell.add_block`.
    pub fn cell_add_block(&mut self, cell: CellId, block: BlockId) {
        self.cell_mut(cell).items.push(ItemId::Block(block));
    }

    /// Port of `Cell.add_table`.
    pub fn cell_add_table(&mut self, cell: CellId, table: TableId) {
        self.cell_mut(cell).items.push(ItemId::Table(table));
    }

    /// Rebuilds a `Style` from a stored `orig_tag_style_node`, the
    /// "stored `NodeId`, rebuilt `Style`" pattern -- see
    /// [`Row::orig_tag_style_node`]'s docs.
    fn rebuild_style<'a>(
        node: Option<NodeId>,
        dom: &'a Dom,
        resolved: &'a ResolvedStyles,
        profile: &'a Profile,
    ) -> Option<Style<'a>> {
        node.map(|n| Style::new(dom, resolved, profile, n))
    }

    /// Port of `Row.add_block`: appends `block` to this row's current
    /// cell, auto-creating one from the row's own stored style
    /// (`orig_tag_style_node`) if none is open yet.
    pub fn row_add_block(
        &mut self,
        row: RowId,
        block: BlockId,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
    ) {
        let cell = match self.rows[row.0].current_cell {
            Some(c) => c,
            None => {
                let html_tag = self.rows[row.0].html_tag;
                let style = Self::rebuild_style(
                    self.rows[row.0].orig_tag_style_node,
                    dom,
                    resolved,
                    profile,
                );
                self.row_start_new_cell(row, html_tag, dom, style.as_ref())
            }
        };
        self.cell_add_block(cell, block);
    }

    /// Port of `Row.add_table`: same auto-create-a-cell fallback as
    /// [`Self::row_add_block`].
    pub fn row_add_table(
        &mut self,
        row: RowId,
        table: TableId,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
    ) {
        let cell = match self.rows[row.0].current_cell {
            Some(c) => c,
            None => {
                let html_tag = self.rows[row.0].html_tag;
                let style = Self::rebuild_style(
                    self.rows[row.0].orig_tag_style_node,
                    dom,
                    resolved,
                    profile,
                );
                self.row_start_new_cell(row, html_tag, dom, style.as_ref())
            }
        };
        self.cell_add_table(cell, table);
    }

    /// Port of `Table.start_new_row`: any unfinished current row is
    /// pushed into `rows` as-is (matching Python -- a row still
    /// missing its own closing tag when a *new* row starts just gets
    /// whatever `cells` it had accumulated so far; nothing here
    /// forces it closed).
    pub fn table_start_new_row(
        &mut self,
        table: TableId,
        html_tag: NodeId,
        tag_style: Option<&Style>,
    ) -> RowId {
        if let Some(prev) = self.tables[table.0].current_row.take() {
            self.tables[table.0].rows.push(prev);
        }
        let id = RowId(self.rows.len());
        self.rows.push(Row::new(table, html_tag, tag_style));
        self.tables[table.0].current_row = Some(id);
        id
    }

    /// Port of `Table.start_new_cell`.
    pub fn table_start_new_cell(
        &mut self,
        table: TableId,
        html_tag: NodeId,
        dom: &Dom,
        tag_style: Option<&Style>,
    ) {
        let row = match self.tables[table.0].current_row {
            Some(r) => r,
            None => self.table_start_new_row(table, html_tag, None),
        };
        self.row_start_new_cell(row, html_tag, dom, tag_style);
    }

    /// Port of `Table.finish_tag`. Returns whether `html_tag` was the
    /// TABLE's own opening tag closing -- [`Self::table_finish_tag`]'s
    /// caller (the still-unwired `Blocks.finish_tag` table branch)
    /// uses this to decide whether the table itself is done and ready
    /// to move into its parent container. When the table itself
    /// closes, this also runs [`Self::expand_spanned_cells`] and
    /// [`Self::resolve_cell_borders`] for every real cell -- Python
    /// calls `cell.resolve_borders()` unconditionally (a no-op for
    /// `SpannedCell`); this skips `Spanned` slots instead, since this
    /// port's [`Self::resolve_cell_borders`] panics on one rather than
    /// silently doing nothing.
    pub fn table_finish_tag(&mut self, table: TableId, html_tag: NodeId) -> bool {
        if let Some(row) = self.tables[table.0].current_row {
            self.row_finish_tag(row, html_tag);
            if self.rows[row.0].html_tag == html_tag {
                self.tables[table.0].rows.push(row);
                self.tables[table.0].current_row = None;
            }
        }
        let table_ended = self.tables[table.0].html_tag == html_tag;
        if table_ended {
            self.expand_spanned_cells(table);
            let row_ids = self.tables[table.0].rows.clone();
            for row_id in row_ids {
                let cell_ids = self.rows[row_id.0].cells.clone();
                for cell_id in cell_ids {
                    if matches!(self.cell_slot(cell_id), CellSlot::Cell(_)) {
                        self.resolve_cell_borders(cell_id);
                    }
                }
            }
        }
        table_ended
    }

    /// Port of `Table.add_block`. Panics if no row is open -- matches
    /// Python's own unguarded `self.current_row.add_block(block)`
    /// (`Blocks.end_current_block`, the only real caller, never
    /// routes a block into a table without one, since it only does so
    /// when `current_table.current_row is not None`).
    pub fn table_add_block(
        &mut self,
        table: TableId,
        block: BlockId,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
    ) {
        let row = self.tables[table.0]
            .current_row
            .expect("Table.add_block assumes a current row, matching Python");
        self.row_add_block(row, block, dom, resolved, profile);
    }

    /// Port of `Table.add_table`: auto-creates a row from the table's
    /// own stored style if none is open, the same fallback shape as
    /// [`Self::row_add_block`]'s cell fallback.
    pub fn table_add_table(
        &mut self,
        table: TableId,
        nested: TableId,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
    ) {
        let row = match self.tables[table.0].current_row {
            Some(r) => r,
            None => {
                let html_tag = self.tables[table.0].html_tag;
                let style = Self::rebuild_style(
                    self.tables[table.0].orig_tag_style_node,
                    dom,
                    resolved,
                    profile,
                );
                self.table_start_new_row(table, html_tag, style.as_ref())
            }
        };
        self.row_add_table(row, nested, dom, resolved, profile);
    }

    /// Port of `Cell.serialize`. `serialize_block` is the one piece of
    /// context this arena genuinely can't provide on its own -- a real
    /// `Block`'s content needs `StylesManager`/`LinksManager` (both
    /// owned by `Blocks`, in `from_html.rs`), so rendering one is a
    /// caller-supplied callback rather than a parameter this method
    /// resolves itself. A nested [`ItemId::Table`] needs no such
    /// callback -- it recurses straight back into
    /// [`Self::serialize_table`], since everything a nested table
    /// needs already lives in this same arena.
    pub fn serialize_cell(
        &self,
        cell_id: CellId,
        parent: &mut Element,
        serialize_block: &mut dyn FnMut(&mut Element, BlockId),
    ) {
        let cell = self.cell(cell_id);
        let row = self.row(cell.row);
        let table = self.table(cell.table);

        let tc = parent.append(Element::new("w:tc"));
        let tc_pr = tc.append(Element::new("w:tcPr"));
        tc_pr.append(
            Element::new("w:tcW")
                .attr("w:type", cell.width.0)
                .attr("w:w", cell.width.1.to_string()),
        );

        // Word 2007 refuses to honor <w:shd> at the table or row
        // level, so it's inherited and applied at the cell level
        // instead -- matches Python's own comment on this exactly.
        let bc = cell
            .background_color
            .clone()
            .or_else(|| row.background_color.clone())
            .or_else(|| table.background_color.clone());
        if let Some(bc) = bc {
            tc_pr.append(
                Element::new("w:shd")
                    .attr("w:val", "clear")
                    .attr("w:color", "auto")
                    .attr("w:fill", bc),
            );
        }

        let mut borders = Element::new("w:tcBorders");
        if let Some(resolved) = &cell.resolved_borders {
            for (edge, border) in [
                ("left", &resolved.left),
                ("top", &resolved.top),
                ("right", &resolved.right),
                ("bottom", &resolved.bottom),
            ] {
                if let Some(border) = border {
                    if border.width > 0 && border.style != "none" {
                        let mut el = Element::new(format!("w:{edge}"))
                            .attr("w:val", border.style.clone())
                            .attr("w:sz", border.width.to_string());
                        if let Some(color) = &border.color {
                            el = el.attr("w:color", color.clone());
                        }
                        borders.append(el);
                    }
                }
            }
        }
        if !borders.is_empty() {
            tc_pr.append(borders);
        }

        let mut margins = Element::new("w:tcMar");
        let first_cell = row.first_cell();
        let last_cell = row.last_cell();
        for edge in BORDER_EDGES {
            let mut padding = cell.borders.padding(edge);
            let inherits_from_row = match edge {
                "top" | "bottom" => true,
                "left" => first_cell == Some(cell_id),
                "right" => last_cell == Some(cell_id),
                _ => false,
            };
            if inherits_from_row {
                padding += row.borders.padding(edge);
            }
            if padding > 0 {
                margins.append(
                    Element::new(format!("w:{edge}"))
                        .attr("w:type", "dxa")
                        .attr("w:w", (padding * 20).to_string()),
                );
            }
        }
        if !margins.is_empty() {
            tc_pr.append(margins);
        }

        if let Some(valign) = cell.valign {
            tc_pr.append(Element::new("w:vAlign").attr("w:val", valign));
        }
        if cell.row_span > 1 {
            tc_pr.append(Element::new("w:vMerge").attr("w:val", "restart"));
        }
        if cell.col_span > 1 {
            tc_pr.append(Element::new("w:hMerge").attr("w:val", "restart"));
        }

        let mut last_was_table = false;
        let mut had_item = false;
        for &item in &cell.items {
            had_item = true;
            match item {
                ItemId::Block(id) => {
                    last_was_table = false;
                    serialize_block(tc, id);
                }
                ItemId::Table(id) => {
                    last_was_table = true;
                    self.serialize_table(id, tc, serialize_block);
                }
            }
        }
        if !had_item || last_was_table {
            // Word 2007 requires the last element in a table cell to
            // be a paragraph.
            tc.append(Element::new("w:p"));
        }
    }

    /// Port of `Row.serialize`.
    pub fn serialize_row(
        &self,
        row: RowId,
        parent: &mut Element,
        serialize_block: &mut dyn FnMut(&mut Element, BlockId),
    ) {
        let tr = parent.append(Element::new("w:tr"));
        for &cell_id in &self.row(row).cells {
            match self.cell_slot(cell_id) {
                CellSlot::Spanned(s) => s.serialize(tr),
                CellSlot::Cell(_) => self.serialize_cell(cell_id, tr, serialize_block),
            }
        }
    }

    /// Port of `Table.serialize`. Rows with no cells at all (matching
    /// Python's `[r for r in self.rows if r.cells]`) are skipped
    /// entirely, and the table itself emits nothing if that leaves no
    /// rows.
    pub fn serialize_table(
        &self,
        table_id: TableId,
        parent: &mut Element,
        serialize_block: &mut dyn FnMut(&mut Element, BlockId),
    ) {
        let table = self.table(table_id);
        let row_ids: Vec<RowId> = table
            .rows
            .iter()
            .copied()
            .filter(|&r| !self.row(r).cells.is_empty())
            .collect();
        if row_ids.is_empty() {
            return;
        }

        let tbl = parent.append(Element::new("w:tbl"));
        let tbl_pr = tbl.append(Element::new("w:tblPr"));
        tbl_pr.append(
            Element::new("w:tblW")
                .attr("w:type", table.width.0)
                .attr("w:w", table.width.1.to_string()),
        );
        if let Some(float_dir) = table.float.as_deref() {
            if float_dir == "left" || float_dir == "right" {
                let mut tblp_pr = Element::new("w:tblpPr")
                    .attr("w:vertAnchor", "text")
                    .attr("w:horzAnchor", "text")
                    .attr("w:tblpXSpec", float_dir);
                for edge in BORDER_EDGES {
                    let mut val = match edge {
                        "left" => table.margin_left,
                        "top" => table.margin_top,
                        "right" => table.margin_right,
                        "bottom" => table.margin_bottom,
                        _ => None,
                    }
                    .unwrap_or(0.0);
                    let is_opposite_edge = (float_dir == "left" && edge == "right")
                        || (float_dir == "right" && edge == "left");
                    if is_opposite_edge {
                        val = val.max(2.0);
                    }
                    let twips = ((val * 20.0) as i64).max(0);
                    tblp_pr.set(format!("w:{edge}FromText"), twips.to_string());
                }
                tbl_pr.append(tblp_pr);
            }
        }
        if let Some(jc) = table.jc {
            tbl_pr.append(Element::new("w:jc").attr("w:val", jc));
        }

        for row_id in row_ids {
            self.serialize_row(row_id, tbl, serialize_block);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{Dom, NodeId};
    use crate::oeb::polish::cascade::{PropertyValue, ResolvedStyles};
    use crate::oeb::polish::style::Profile;
    use std::collections::HashMap;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn resolved_with(entries: &[(NodeId, &[(&str, &str)])]) -> ResolvedStyles {
        let mut style_map = HashMap::new();
        for &(id, props) in entries {
            let mut m = HashMap::new();
            for &(k, v) in props {
                m.insert(k.to_string(), PropertyValue::new(v, None, false));
            }
            style_map.insert(id, m);
        }
        ResolvedStyles {
            style_map,
            pseudo_style_map: HashMap::new(),
        }
    }

    fn find(dom: &Dom, tag: &str) -> NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    #[test]
    fn border_style_weight_orders_double_highest_and_unknown_lowest() {
        assert!(border_style_weight("double") > border_style_weight("solid"));
        assert!(border_style_weight("solid") > border_style_weight("inset"));
        assert_eq!(border_style_weight("none"), 0);
        assert_eq!(border_style_weight("garbage"), 0);
    }

    #[test]
    fn as_percent_parses_a_percent_suffix() {
        assert_eq!(as_percent("50%"), Some(50.0));
        assert_eq!(as_percent("50"), None);
        assert_eq!(as_percent("auto"), None);
        assert_eq!(as_percent(""), None);
    }

    #[test]
    fn convert_width_with_no_style_is_auto() {
        assert_eq!(convert_width(None), ("auto", 0));
    }

    #[test]
    fn convert_width_auto_keyword() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("width", "auto")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(convert_width(Some(&style)), ("auto", 0));
    }

    #[test]
    fn convert_width_percent() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("width", "50%")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(convert_width(Some(&style)), ("pct", 2500));
    }

    #[test]
    fn convert_width_absolute_length_uses_dxa() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("width", "50pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(convert_width(Some(&style)), ("dxa", 1000));
    }

    #[test]
    fn read_css_block_borders_tags_every_edge_with_the_given_level() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(
            table,
            &[
                ("border-left-style", "solid"),
                ("border-left-width", "2px"),
                ("border-left-color", "#ff0000"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        let borders = read_css_block_borders(Some(&style), 0);
        assert_eq!(borders.left.level, 0);
        assert_eq!(borders.left.style, "single");
        assert_eq!(borders.left.css_style, "solid");
        assert!(borders.left.width > 0);
        assert_eq!(borders.top.style, "none");
    }

    #[test]
    fn read_css_block_borders_with_no_css_still_returns_defaults() {
        let borders = read_css_block_borders(None, 2);
        assert_eq!(borders.left.level, 2);
        assert_eq!(borders.left.color, None);
    }

    #[test]
    fn edge_borders_border_and_padding_accessors_dispatch_by_edge_name() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("padding-right", "5pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        let borders = read_css_block_borders(Some(&style), 0);
        assert_eq!(borders.padding("right"), borders.padding_right);
        assert!(std::ptr::eq(borders.border("top"), &borders.top));
    }

    #[test]
    fn table_background_color_is_none_with_no_style() {
        assert_eq!(table_background_color(None), None);
    }

    #[test]
    fn table_background_color_normalizes_a_named_color() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("background-color", "red")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(
            table_background_color(Some(&style)),
            Some("FF0000".to_string())
        );
    }

    fn find_all(dom: &Dom, tag: &str) -> Vec<NodeId> {
        dom.preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some(tag))
            .collect()
    }

    #[test]
    fn cell_new_reads_rowspan_and_colspan() {
        let dom = make(
            r#"<html><body><table><tr><td rowspan="2" colspan="3">x</td></tr></table></body></html>"#,
        );
        let td = find(&dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(find(&dom, "table"), None);
        let r = tables.add_row(t, find(&dom, "tr"), None);
        let c = tables.add_cell(r, td, &dom, None);
        assert_eq!(tables.cell(c).row_span, 2);
        assert_eq!(tables.cell(c).col_span, 3);
    }

    #[test]
    fn cell_new_defaults_span_to_one_when_absent_or_invalid() {
        let dom = make(
            r#"<html><body><table><tr><td>a</td><td rowspan="abc">b</td></tr></table></body></html>"#,
        );
        let tds = find_all(&dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(find(&dom, "table"), None);
        let r = tables.add_row(t, find(&dom, "tr"), None);
        let c1 = tables.add_cell(r, tds[0], &dom, None);
        let c2 = tables.add_cell(r, tds[1], &dom, None);
        assert_eq!(tables.cell(c1).row_span, 1);
        assert_eq!(
            tables.cell(c2).row_span,
            1,
            "unparseable value falls back to 1"
        );
    }

    #[test]
    fn cell_new_clamps_negative_span_to_zero() {
        let dom =
            make(r#"<html><body><table><tr><td rowspan="-5">a</td></tr></table></body></html>"#);
        let td = find(&dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(find(&dom, "table"), None);
        let r = tables.add_row(t, find(&dom, "tr"), None);
        let c = tables.add_cell(r, td, &dom, None);
        assert_eq!(tables.cell(c).row_span, 0);
    }

    #[test]
    fn cell_new_valign_maps_keywords_and_drops_the_rest() {
        let dom = make("<html><body><table><tr><td>a</td></tr></table></body></html>");
        let td = find(&dom, "td");
        let profile = Profile::default();

        let resolved = resolved_with(&[(td, &[("vertical-align", "top")])]);
        let style = Style::new(&dom, &resolved, &profile, td);
        let mut tables = Tables::new();
        let t = tables.add_table(find(&dom, "table"), None);
        let r = tables.add_row(t, find(&dom, "tr"), None);
        let c = tables.add_cell(r, td, &dom, Some(&style));
        assert_eq!(tables.cell(c).valign, Some("top"));

        let resolved = resolved_with(&[(td, &[("vertical-align", "middle")])]);
        let style = Style::new(&dom, &resolved, &profile, td);
        let c = tables.add_cell(r, td, &dom, Some(&style));
        assert_eq!(tables.cell(c).valign, Some("center"));

        let resolved = resolved_with(&[(td, &[("vertical-align", "baseline")])]);
        let style = Style::new(&dom, &resolved, &profile, td);
        let c = tables.add_cell(r, td, &dom, Some(&style));
        assert_eq!(
            tables.cell(c).valign,
            None,
            "an unrecognized keyword drops out, unlike a wholly absent tag_style"
        );

        let c = tables.add_cell(r, td, &dom, None);
        assert_eq!(
            tables.cell(c).valign,
            Some("center"),
            "no tag_style at all defaults to center"
        );
    }

    #[test]
    fn table_new_with_no_style_leaves_jc_float_and_margins_unset() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let t = Table::new(table, None);
        assert_eq!(t.jc, None);
        assert_eq!(t.float, None);
        assert_eq!(t.margin_left, None);
    }

    #[test]
    fn table_new_both_auto_margins_center_one_auto_right_aligns() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let profile = Profile::default();

        let resolved =
            resolved_with(&[(table, &[("margin-left", "auto"), ("margin-right", "auto")])]);
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(Table::new(table, Some(&style)).jc, Some("center"));

        let resolved = resolved_with(&[(table, &[("margin-left", "auto")])]);
        let style = Style::new(&dom, &resolved, &profile, table);
        assert_eq!(Table::new(table, Some(&style)).jc, Some("right"));
    }

    #[test]
    fn table_new_reads_float_and_numeric_margins() {
        let dom = make("<html><body><table/></body></html>");
        let table = find(&dom, "table");
        let resolved = resolved_with(&[(table, &[("float", "left"), ("margin-top", "5pt")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table);
        let t = Table::new(table, Some(&style));
        assert_eq!(t.float.as_deref(), Some("left"));
        assert_eq!(t.margin_top, Some(5.0));
    }

    fn build_grid(dom: &Dom, rows: usize, cols: usize) -> (Tables, TableId, Vec<Vec<CellId>>) {
        let table_tag = find(dom, "table");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let mut grid = Vec::new();
        for _ in 0..rows {
            let r = tables.add_row(t, table_tag, None);
            let mut row_cells = Vec::new();
            for _ in 0..cols {
                row_cells.push(tables.add_cell(r, table_tag, dom, None));
            }
            grid.push(row_cells);
        }
        (tables, t, grid)
    }

    #[test]
    fn tables_add_wires_row_and_cell_back_references() {
        let dom = make("<html><body><table/></body></html>");
        let (tables, t, grid) = build_grid(&dom, 2, 2);
        let c = grid[0][1];
        assert_eq!(tables.cell(c).table, t);
        assert_eq!(tables.table(t).rows.len(), 2);
        let r = tables.cell(c).row;
        assert_eq!(tables.row(r).cells, grid[0]);
    }

    #[test]
    fn neighbor_left_and_right_within_a_row() {
        let dom = make("<html><body><table/></body></html>");
        let (tables, _t, grid) = build_grid(&dom, 1, 3);
        assert_eq!(tables.neighbor(grid[0][1], "left"), Some(grid[0][0]));
        assert_eq!(tables.neighbor(grid[0][1], "right"), Some(grid[0][2]));
        assert_eq!(tables.neighbor(grid[0][0], "left"), None);
        assert_eq!(tables.neighbor(grid[0][2], "right"), None);
    }

    #[test]
    fn neighbor_top_and_bottom_across_rows() {
        let dom = make("<html><body><table/></body></html>");
        let (tables, _t, grid) = build_grid(&dom, 3, 2);
        assert_eq!(tables.neighbor(grid[1][0], "top"), Some(grid[0][0]));
        assert_eq!(tables.neighbor(grid[1][0], "bottom"), Some(grid[2][0]));
        assert_eq!(tables.neighbor(grid[0][0], "top"), None);
        assert_eq!(tables.neighbor(grid[2][0], "bottom"), None);
    }

    #[test]
    fn applicable_borders_first_cell_includes_table_and_row_left_border() {
        let dom = make("<html><body><table/></body></html>");
        let (tables, _t, grid) = build_grid(&dom, 1, 2);
        assert_eq!(tables.applicable_borders(grid[0][0], "left").len(), 3);
        assert_eq!(
            tables.applicable_borders(grid[0][1], "left").len(),
            1,
            "a non-edge cell contributes only its own border"
        );
    }

    #[test]
    fn applicable_borders_top_gates_on_the_row_not_the_cell_position() {
        let dom = make("<html><body><table/></body></html>");
        let (tables, _t, grid) = build_grid(&dom, 2, 2);
        // Every cell in the first row includes the table's top border,
        // regardless of its own horizontal position -- unlike left/right,
        // which gate on being the row's first/last cell.
        assert_eq!(tables.applicable_borders(grid[0][0], "top").len(), 3);
        assert_eq!(tables.applicable_borders(grid[0][1], "top").len(), 3);
        assert_eq!(tables.applicable_borders(grid[1][0], "top").len(), 2);
    }

    fn styled_border(css_style: &str, width: i64, level: u8) -> Border {
        Border {
            css_style: css_style.to_string(),
            style: css_style.to_string(),
            width,
            color: None,
            level,
        }
    }

    #[test]
    fn resolve_border_prefers_wider_over_narrower() {
        let dom = make("<html><body><table><tr><td>a</td></tr></table></body></html>");
        let table_tag = find(&dom, "table");
        let td = find(&dom, "td");
        let profile = Profile::default();
        let resolved = resolved_with(&[(
            td,
            &[("border-left-style", "solid"), ("border-left-width", "1pt")],
        )]);
        let style = Style::new(&dom, &resolved, &profile, td);
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.add_row(t, table_tag, None);
        let c = tables.add_cell(r, td, &dom, Some(&style));
        let resolved = tables.resolve_border(c, "left").unwrap();
        assert_eq!(resolved.css_style, "solid");

        // A cell whose own border is wider should win over a thin one,
        // even though both are the same style/level.
        let mut tables2 = Tables::new();
        let t2 = tables2.add_table(table_tag, None);
        let r2 = tables2.add_row(t2, table_tag, None);
        let c2 = tables2.add_cell(r2, td, &dom, None);
        if let CellSlot::Cell(cell) = &mut tables2.cells[c2.0] {
            cell.borders.left = styled_border("solid", 40, 2);
        }
        assert_eq!(tables2.resolve_border(c2, "left").unwrap().width, 40);
    }

    #[test]
    fn resolve_border_any_hidden_side_vetoes_the_whole_edge() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 2);
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][0].0] {
            cell.borders.right = styled_border("solid", 20, 2);
        }
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][1].0] {
            cell.borders.left = styled_border("hidden", 0, 2);
        }
        assert_eq!(tables.resolve_border(grid[0][0], "right"), None);
    }

    #[test]
    fn resolve_border_prefers_the_more_specific_level_when_otherwise_tied() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 1);
        // Table-level (level 0) declares a border; the cell (level 2)
        // declares an identically-weighted one except for its higher
        // level, which should win the tie.
        tables.tables[t.0].borders.left = styled_border("solid", 10, 0);
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][0].0] {
            cell.borders.left = styled_border("solid", 10, 2);
        }
        assert_eq!(tables.resolve_border(grid[0][0], "left").unwrap().level, 2);
    }

    #[test]
    fn expand_spanned_cells_horizontal_inserts_one_placeholder_per_extra_column() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 2);
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][0].0] {
            cell.col_span = 3;
        }
        tables.expand_spanned_cells(t);
        let row = tables.row(tables.cell(grid[0][0]).row);
        // The row should now have 4 physical slots: the spanning cell,
        // two horizontal placeholders, then the original second cell.
        assert_eq!(row.cells.len(), 4);
        assert!(
            matches!(tables.cell_slot(row.cells[1]), CellSlot::Spanned(s) if s.horizontal && s.spanning_cell == grid[0][0])
        );
        assert!(
            matches!(tables.cell_slot(row.cells[2]), CellSlot::Spanned(s) if s.horizontal && s.spanning_cell == grid[0][0])
        );
        assert_eq!(row.cells[3], grid[0][1]);
    }

    #[test]
    fn expand_spanned_cells_vertical_inserts_placeholders_into_following_rows() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 3, 1);
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][0].0] {
            cell.row_span = 3;
        }
        tables.expand_spanned_cells(t);
        assert!(matches!(
            tables.cell_slot(tables.row(tables.cell(grid[1][0]).row).cells[0]),
            CellSlot::Spanned(_)
        ));
        let r1 = tables.table(t).rows[1];
        let r2 = tables.table(t).rows[2];
        assert!(
            matches!(tables.cell_slot(tables.row(r1).cells[0]), CellSlot::Spanned(s) if s.spanning_cell == grid[0][0])
        );
        assert!(
            matches!(tables.cell_slot(tables.row(r2).cells[0]), CellSlot::Spanned(s) if s.spanning_cell == grid[0][0])
        );
    }

    // The stateful start_new_row/start_new_cell/finish_tag/add_block/
    // add_table methods below are tested against synthetic,
    // disconnected nodes (`Dom::empty` + `Dom::new_element`) rather
    // than parsed HTML fragments -- `<table><td>` without a `<tr>` is
    // exactly the malformed-markup case these fallbacks exist for,
    // and html5ever's real tree-construction rules would silently
    // reparent it (a known gotcha from this effort's `from_html.rs`
    // tests). `Cell`/`Row`/`Table::new` only ever read the given
    // node's own attrs, never its ancestry, so a disconnected node is
    // just as real an input as a parsed one here.

    fn fresh_dom() -> Dom {
        Dom::empty()
    }

    fn new_tag(dom: &mut Dom, name: &str) -> NodeId {
        dom.new_element(name)
    }

    fn new_tag_with_attr(dom: &mut Dom, name: &str, attr: &str, value: &str) -> NodeId {
        let id = dom.new_element(name);
        dom.node_mut(id)
            .attrs
            .insert(attr.to_string(), value.to_string());
        id
    }

    #[test]
    fn table_start_new_row_pushes_an_unfinished_row_into_rows_first() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr1 = new_tag(&mut dom, "tr");
        let tr2 = new_tag(&mut dom, "tr");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r1 = tables.table_start_new_row(t, tr1, None);
        let r2 = tables.table_start_new_row(t, tr2, None);
        assert_eq!(tables.table(t).current_row, Some(r2));
        assert_eq!(
            tables.table(t).rows,
            vec![r1],
            "the unfinished first row is pushed in as-is, not dropped"
        );
    }

    #[test]
    fn table_start_new_cell_auto_starts_a_row_when_none_is_open() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let td_tag = new_tag(&mut dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        tables.table_start_new_cell(t, td_tag, &dom, None);
        let row = tables.table(t).current_row.expect("a row was auto-created");
        assert_eq!(
            tables.row(row).html_tag,
            td_tag,
            "the fallback row reuses the cell's own html_tag, matching \
             Python's start_new_row(html_tag, None)"
        );
        assert!(tables.row(row).current_cell.is_some());
    }

    #[test]
    fn table_finish_tag_closes_the_open_cell_and_row_on_matching_tags() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let td_tag = new_tag(&mut dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.table_start_new_row(t, tr_tag, None);
        tables.row_start_new_cell(r, td_tag, &dom, None);

        assert!(!tables.table_finish_tag(t, td_tag));
        assert_eq!(tables.row(r).current_cell, None);
        assert_eq!(tables.row(r).cells.len(), 1);

        assert!(!tables.table_finish_tag(t, tr_tag));
        assert_eq!(tables.table(t).current_row, None);
        assert_eq!(tables.table(t).rows, vec![r]);

        assert!(
            tables.table_finish_tag(t, table_tag),
            "only the table's own opening tag closing returns true"
        );
    }

    #[test]
    fn table_finish_tag_on_table_close_runs_expand_and_resolve_borders() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let td1 = new_tag_with_attr(&mut dom, "td", "colspan", "2");
        let td2 = new_tag(&mut dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.table_start_new_row(t, tr_tag, None);
        tables.row_start_new_cell(r, td1, &dom, None);
        tables.table_finish_tag(t, td1);
        tables.row_start_new_cell(r, td2, &dom, None);
        tables.table_finish_tag(t, td2);
        tables.table_finish_tag(t, tr_tag);

        assert!(tables.table_finish_tag(t, table_tag));

        let row = tables.table(t).rows[0];
        assert_eq!(
            tables.row(row).cells.len(),
            3,
            "the colspan=2 cell should have expanded to include one placeholder"
        );
        assert!(tables
            .cell(tables.row(row).cells[0])
            .resolved_borders
            .is_some());
        assert!(tables
            .cell(tables.row(row).cells[2])
            .resolved_borders
            .is_some());
    }

    #[test]
    fn cell_add_block_and_add_table_append_to_items_in_order() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let td_tag = new_tag(&mut dom, "td");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.add_row(t, tr_tag, None);
        let c = tables.add_cell(r, td_tag, &dom, None);
        let block = BlockId(7);
        let nested = tables.add_table(table_tag, None);
        tables.cell_add_block(c, block);
        tables.cell_add_table(c, nested);
        assert_eq!(
            tables.cell(c).items,
            vec![ItemId::Block(block), ItemId::Table(nested)]
        );
    }

    #[test]
    fn row_add_block_auto_creates_a_cell_from_the_rows_own_style_when_none_is_open() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let resolved = resolved_with(&[(tr_tag, &[("background-color", "red")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, tr_tag);
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.add_row(t, tr_tag, Some(&style));
        assert_eq!(tables.row(r).current_cell, None);

        let block = BlockId(3);
        tables.row_add_block(r, block, &dom, &resolved, &profile);

        let cell = tables.row(r).current_cell.expect("a cell was auto-created");
        assert_eq!(
            tables.cell(cell).html_tag,
            tr_tag,
            "the fallback cell reuses the row's own html_tag"
        );
        assert_eq!(
            tables.cell(cell).background_color.as_deref(),
            Some("FF0000"),
            "the auto-created cell picks up the row's own stored style"
        );
        assert_eq!(tables.cell(cell).items, vec![ItemId::Block(block)]);
    }

    #[test]
    fn row_add_table_auto_creates_a_cell_when_none_is_open() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        let r = tables.add_row(t, tr_tag, None);
        let nested = tables.add_table(table_tag, None);

        tables.row_add_table(r, nested, &dom, &resolved, &profile);

        let cell = tables.row(r).current_cell.expect("a cell was auto-created");
        assert_eq!(tables.cell(cell).items, vec![ItemId::Table(nested)]);
    }

    #[test]
    fn table_add_block_routes_through_the_current_row() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let tr_tag = new_tag(&mut dom, "tr");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        tables.table_start_new_row(t, tr_tag, None);

        let block = BlockId(9);
        tables.table_add_block(t, block, &dom, &resolved, &profile);

        let row = tables.table(t).current_row.unwrap();
        let cell = tables.row(row).current_cell.unwrap();
        assert_eq!(tables.cell(cell).items, vec![ItemId::Block(block)]);
    }

    #[test]
    #[should_panic(expected = "Table.add_block assumes a current row")]
    fn table_add_block_panics_when_no_row_is_open() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        tables.table_add_block(t, BlockId(1), &dom, &resolved, &profile);
    }

    #[test]
    fn table_add_table_auto_creates_a_row_from_the_tables_own_style_when_none_is_open() {
        let mut dom = fresh_dom();
        let table_tag = new_tag(&mut dom, "table");
        let resolved = resolved_with(&[(table_tag, &[("background-color", "blue")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, table_tag);
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, Some(&style));
        assert_eq!(tables.table(t).current_row, None);
        let nested = tables.add_table(table_tag, None);

        tables.table_add_table(t, nested, &dom, &resolved, &profile);

        let row = tables.table(t).current_row.expect("a row was auto-created");
        assert_eq!(
            tables.row(row).html_tag,
            table_tag,
            "the fallback row reuses the table's own html_tag"
        );
        assert_eq!(
            tables.row(row).background_color.as_deref(),
            Some("0000FF"),
            "the auto-created row picks up the table's own stored style"
        );
        let cell = tables
            .row(row)
            .current_cell
            .expect("a cell was auto-created");
        assert_eq!(tables.cell(cell).items, vec![ItemId::Table(nested)]);
    }

    #[test]
    fn spanned_cell_serialize_emits_a_horizontal_merge_marker() {
        let sc = SpannedCell {
            spanning_cell: CellId(0),
            horizontal: true,
        };
        let mut parent = Element::new("w:tr");
        sc.serialize(&mut parent);
        let tc = parent.children_named("w:tc").next().unwrap();
        let tc_pr = tc.children_named("w:tcPr").next().unwrap();
        assert_eq!(
            tc_pr
                .children_named("w:hMerge")
                .next()
                .unwrap()
                .get("w:val"),
            Some("continue")
        );
        assert!(tc.children_named("w:p").next().is_some());
    }

    #[test]
    fn spanned_cell_serialize_vertical_uses_v_merge() {
        let sc = SpannedCell {
            spanning_cell: CellId(0),
            horizontal: false,
        };
        let mut parent = Element::new("w:tr");
        sc.serialize(&mut parent);
        let tc_pr = parent
            .children_named("w:tc")
            .next()
            .unwrap()
            .children_named("w:tcPr")
            .next()
            .unwrap();
        assert!(tc_pr.children_named("w:vMerge").next().is_some());
    }

    fn no_block(_: &mut Element, _: BlockId) {}

    #[test]
    fn serialize_cell_emits_width_and_a_paragraph_when_empty() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 1);
        let cell = grid[0][0];
        tables.resolve_cell_borders(cell);
        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);
        let tc = parent.children_named("w:tc").next().unwrap();
        let tc_pr = tc.children_named("w:tcPr").next().unwrap();
        let tc_w = tc_pr.children_named("w:tcW").next().unwrap();
        assert_eq!(tc_w.get("w:type"), Some("auto"));
        assert!(
            tc.children_named("w:p").next().is_some(),
            "an empty cell still needs a trailing paragraph"
        );
    }

    #[test]
    fn serialize_cell_writes_resolved_border_edges() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 1);
        let cell = grid[0][0];
        if let CellSlot::Cell(c) = &mut tables.cells[cell.0] {
            c.borders.left = styled_border("solid", 20, 2);
        }
        tables.resolve_cell_borders(cell);
        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);
        let tc_pr = parent
            .children_named("w:tc")
            .next()
            .unwrap()
            .children_named("w:tcPr")
            .next()
            .unwrap();
        let borders = tc_pr.children_named("w:tcBorders").next().unwrap();
        let left = borders.children_named("w:left").next().unwrap();
        assert_eq!(left.get("w:val"), Some("solid"));
        assert_eq!(left.get("w:sz"), Some("20"));
    }

    #[test]
    fn serialize_cell_inherits_background_from_row_then_table() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 1);
        let cell = grid[0][0];
        tables.tables[t.0].background_color = Some("0000FF".to_string());
        tables.resolve_cell_borders(cell);
        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);
        let tc_pr = parent
            .children_named("w:tc")
            .next()
            .unwrap()
            .children_named("w:tcPr")
            .next()
            .unwrap();
        let shd = tc_pr.children_named("w:shd").next().unwrap();
        assert_eq!(shd.get("w:fill"), Some("0000FF"));
    }

    #[test]
    fn serialize_cell_adds_row_padding_on_the_left_for_the_rows_first_cell() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 2);
        let cell = grid[0][0];
        let row = tables.cell(cell).row;
        tables.rows[row.0].borders.padding_left = 100;
        if let CellSlot::Cell(c) = &mut tables.cells[cell.0] {
            c.borders.padding_left = 50;
        }
        tables.resolve_cell_borders(cell);
        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);
        let margins = parent
            .children_named("w:tc")
            .next()
            .unwrap()
            .children_named("w:tcPr")
            .next()
            .unwrap()
            .children_named("w:tcMar")
            .next()
            .unwrap();
        let left = margins.children_named("w:left").next().unwrap();
        assert_eq!(
            left.get("w:w"),
            Some("3000"),
            "the row's own left padding is added since this is the row's first cell: (50+100)*20"
        );
    }

    #[test]
    fn serialize_cell_does_not_add_row_padding_on_the_left_for_a_non_edge_cell() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 2);
        let cell = grid[0][1];
        let row = tables.cell(cell).row;
        tables.rows[row.0].borders.padding_left = 100;
        if let CellSlot::Cell(c) = &mut tables.cells[cell.0] {
            c.borders.padding_left = 50;
        }
        tables.resolve_cell_borders(cell);
        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);
        let margins = parent
            .children_named("w:tc")
            .next()
            .unwrap()
            .children_named("w:tcPr")
            .next()
            .unwrap()
            .children_named("w:tcMar")
            .next()
            .unwrap();
        let left = margins.children_named("w:left").next().unwrap();
        assert_eq!(
            left.get("w:w"),
            Some("1000"),
            "not the row's first cell, so only its own padding counts: 50*20"
        );
    }

    #[test]
    fn serialize_cell_calls_serialize_block_for_each_block_item_in_order() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, _t, grid) = build_grid(&dom, 1, 1);
        let cell = grid[0][0];
        tables.cell_add_block(cell, BlockId(1));
        tables.cell_add_block(cell, BlockId(2));
        tables.resolve_cell_borders(cell);
        let mut seen = Vec::new();
        let mut parent = Element::new("w:tr");
        {
            let mut cb = |el: &mut Element, id: BlockId| {
                seen.push(id);
                el.append(Element::new("w:p"));
            };
            tables.serialize_cell(cell, &mut parent, &mut cb);
        }
        assert_eq!(seen, vec![BlockId(1), BlockId(2)]);
        let tc = parent.children_named("w:tc").next().unwrap();
        assert_eq!(
            tc.children_named("w:p").count(),
            2,
            "no extra trailing paragraph after a Block item"
        );
    }

    #[test]
    fn serialize_cell_nested_table_recurses_and_still_appends_a_trailing_paragraph() {
        let dom = make("<html><body><table/></body></html>");
        let table_tag = find(&dom, "table");
        let (mut tables, _outer, grid) = build_grid(&dom, 1, 1);
        let cell = grid[0][0];
        let inner = tables.add_table(table_tag, None);
        let inner_row = tables.add_row(inner, table_tag, None);
        tables.add_cell(inner_row, table_tag, &dom, None);
        tables.cell_add_table(cell, inner);
        tables.resolve_cell_borders(cell);

        let mut parent = Element::new("w:tr");
        tables.serialize_cell(cell, &mut parent, &mut no_block);

        let tc = parent.children_named("w:tc").next().unwrap();
        assert!(
            tc.children_named("w:tbl").next().is_some(),
            "the nested table should have been serialized"
        );
        assert!(
            tc.children_named("w:p").next().is_some(),
            "the last item was a Table, so a trailing paragraph is still required"
        );
    }

    #[test]
    fn serialize_row_dispatches_cells_and_spanned_placeholders() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 2);
        if let CellSlot::Cell(cell) = &mut tables.cells[grid[0][0].0] {
            cell.col_span = 2;
        }
        tables.expand_spanned_cells(t);
        let row = tables.cell(grid[0][0]).row;
        let cell_ids: Vec<CellId> = tables.row(row).cells.clone();
        for cell_id in cell_ids {
            if matches!(tables.cell_slot(cell_id), CellSlot::Cell(_)) {
                tables.resolve_cell_borders(cell_id);
            }
        }
        let mut parent = Element::new("w:tbl");
        tables.serialize_row(row, &mut parent, &mut no_block);
        let tr = parent.children_named("w:tr").next().unwrap();
        assert_eq!(
            tr.children_named("w:tc").count(),
            3,
            "1 real cell + 1 horizontal-span placeholder + 1 real cell"
        );
    }

    #[test]
    fn serialize_table_skips_rows_with_no_cells() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 1);
        tables.add_row(t, find(&dom, "table"), None); // an empty row, no cells
        tables.resolve_cell_borders(grid[0][0]);
        let mut parent = Element::new("w:body");
        tables.serialize_table(t, &mut parent, &mut no_block);
        let tbl = parent.children_named("w:tbl").next().unwrap();
        assert_eq!(
            tbl.children_named("w:tr").count(),
            1,
            "the empty row must be skipped entirely"
        );
    }

    #[test]
    fn serialize_table_emits_nothing_when_every_row_is_empty() {
        let dom = make("<html><body><table/></body></html>");
        let table_tag = find(&dom, "table");
        let mut tables = Tables::new();
        let t = tables.add_table(table_tag, None);
        tables.add_row(t, table_tag, None);
        let mut parent = Element::new("w:body");
        tables.serialize_table(t, &mut parent, &mut no_block);
        assert!(parent.children_named("w:tbl").next().is_none());
    }

    #[test]
    fn serialize_table_with_a_float_direction_bumps_the_opposite_edge() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 1);
        tables.tables[t.0].float = Some("left".to_string());
        tables.tables[t.0].margin_left = Some(1.0);
        tables.tables[t.0].margin_right = Some(0.5);
        tables.resolve_cell_borders(grid[0][0]);
        let mut parent = Element::new("w:body");
        tables.serialize_table(t, &mut parent, &mut no_block);
        let tblp_pr = parent
            .children_named("w:tbl")
            .next()
            .unwrap()
            .children_named("w:tblPr")
            .next()
            .unwrap()
            .children_named("w:tblpPr")
            .next()
            .unwrap();
        assert_eq!(tblp_pr.get("w:tblpXSpec"), Some("left"));
        assert_eq!(tblp_pr.get("w:leftFromText"), Some("20"));
        assert_eq!(
            tblp_pr.get("w:rightFromText"),
            Some("40"),
            "the edge opposite the float direction is bumped to a 2pt minimum: max(0.5,2)*20"
        );
    }

    #[test]
    fn serialize_table_with_jc_emits_the_alignment() {
        let dom = make("<html><body><table/></body></html>");
        let (mut tables, t, grid) = build_grid(&dom, 1, 1);
        tables.tables[t.0].jc = Some("center");
        tables.resolve_cell_borders(grid[0][0]);
        let mut parent = Element::new("w:body");
        tables.serialize_table(t, &mut parent, &mut no_block);
        let jc = parent
            .children_named("w:tbl")
            .next()
            .unwrap()
            .children_named("w:tblPr")
            .next()
            .unwrap()
            .children_named("w:jc")
            .next()
            .unwrap();
        assert_eq!(jc.get("w:val"), Some("center"));
    }
}
