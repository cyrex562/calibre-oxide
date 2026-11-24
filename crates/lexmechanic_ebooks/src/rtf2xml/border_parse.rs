//! Port of `old_src/src/calibre/ebooks/rtf2xml/border_parse.py`
//! (`BorderParse`).
//!
//! Parses a single `cw<bd<...`/`cw<bt<...` border-definition line into
//! an attribute dictionary (border side/kind -> value, plus a derived
//! `{border-type}-style` entry picked from a fixed priority order when
//! more than one style keyword is present).
//!
//! This exact logic is *also* independently ported, privately and
//! non-`pub`, three more times in this crate --
//! [`super::paragraph_def`], [`super::styles`] (both `BTreeMap`-based,
//! from the foundation/early-structure issue), and [`super::table`]
//! (`IndexMap`-based, like this module). Those three predate this
//! standalone port and are a deliberate, already-shipped design
//! choice (see each module's own doc for why: keeping each pass's
//! dependency footprint self-contained rather than unifying three
//! independent passes around a shared module after the fact) -- they
//! are intentionally NOT retrofitted to call this module instead.
//! This file exists to satisfy issue #188's own file list, and as the
//! `pub` entry point for any *future* consumer (`group_borders.py`,
//! `border_parse.py`'s other real caller upstream, is out of scope of
//! every currently-tracked issue).
//!
//! Uses `IndexMap` (Python 3.7+ dicts preserve insertion order, and
//! this dictionary is typically written straight into a consumer's
//! output stream attribute-by-attribute) rather than the `BTreeMap`
//! the two older private copies chose.

use indexmap::IndexMap;

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
fn determine_styles(border_type: &str, border_style_list: &[&'static str]) -> IndexMap<String, String> {
    let att = format!("{border_type}-style");
    let mut out = IndexMap::new();
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
    if line.len() >= 20 { &line[20..] } else { "" }
}

/// Port of `BorderParse.parse_border`. Operates directly on an
/// intermediate-format `cw<bd<...`/`cw<bt<...` line (see
/// [`super::process_tokens`]'s module docs) rather than reopening a
/// file.
pub fn parse_border(line: &str) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_border_with_no_value_is_flagged_none() {
        let out = parse_border("cw<bd<bor-cel-ri<nu<");
        assert_eq!(out.get("border-cell-right").map(String::as_str), Some("none"));
    }

    #[test]
    fn a_line_width_and_color_attribute_are_namespaced_under_the_border_type() {
        let out = parse_border("cw<bd<bor-t-r-to<nu<bdr-li-wid:0.50|bdr-color_:2");
        assert_eq!(
            out.get("border-table-row-top-line-width").map(String::as_str),
            Some("0.50")
        );
        assert_eq!(out.get("border-table-row-top-color").map(String::as_str), Some("2"));
    }

    #[test]
    fn a_single_style_keyword_becomes_the_derived_style_attribute() {
        let out = parse_border("cw<bd<bor-par-bo<nu<bdr-dashed");
        assert_eq!(out.get("border-paragraph-bottom-style").map(String::as_str), Some("dashed"));
        // The style keyword itself never produces its own top-level entry.
        assert!(!out.contains_key("border-paragraph-bottom-dashed"));
    }

    #[test]
    fn shadowed_wins_the_priority_order_over_a_later_style_keyword() {
        let out = parse_border("cw<bd<bor-par-bo<nu<bdr-shadow|bdr-dashed");
        assert_eq!(out.get("border-paragraph-bottom-style").map(String::as_str), Some("shadowed"));
    }

    #[test]
    fn an_unrecognized_token_type_returns_an_empty_dict() {
        let out = parse_border("cw<bd<zzzzzzzzzz<nu<true");
        assert!(out.is_empty());
    }
}
