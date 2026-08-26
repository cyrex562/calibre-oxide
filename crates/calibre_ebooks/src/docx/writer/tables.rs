//! Tables (`docx/writer/tables.py`) -- **partial**: only the
//! border/width foundation `Cell`/`Row`/`Table` will need, not those
//! types themselves yet.
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
//! **Not ported yet, deliberately**: [`SpannedCell`], `Cell`, `Row`,
//! `Table` themselves. Unlike `images.py`'s remaining gap, this isn't
//! blocked on a missing subsystem -- it's a real, undecided *design*
//! question in the same family as `StylesManager`'s arena and
//! `BlockId`'s arena: Python's `Cell`/`Row`/`Table` reference each
//! other in both directions (`Cell.row`, `Cell.table`, `Row.cells`,
//! `Table.rows`, plus `Cell.neighbor` climbing across rows and
//! `Cell.applicable_borders` reaching back up to `self.table`/
//! `self.row`), which needs the same arena-of-ids treatment
//! `Blocks`/`StylesManager` already established (`BlockId`-style
//! handles, not direct references) -- but the exact shape (one arena
//! per level? one shared arena with a type tag? how nested tables --
//! `Cell.add_table`, a `Table` *inside* a `Cell` -- fit in) deserves
//! the same dedicated thought those decisions got, not a rushed
//! guess. `Block.parent_items` (dropped when `Block` was ported, PR
//! #333, exactly because nothing needed it without real per-cell item
//! lists) is the concrete thing that becomes real again once `Cell`
//! exists -- expect to revisit that decision then, as already flagged
//! in `Blocks`' own module docs.

use crate::oeb::polish::style::Style;

use super::styles::{read_css_block_borders as read_block_borders, BORDER_EDGES};
use super::utils::convert_color;

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
}
