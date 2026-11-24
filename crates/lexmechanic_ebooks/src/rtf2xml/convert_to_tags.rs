//! Port of `old_src/src/calibre/ebooks/rtf2xml/convert_to_tags.py`
//! (`ConvertToTags`).
//!
//! The pipeline's final pass: converts the bracket-tagged
//! intermediate format's own `mi<tg<...` structural lines into real
//! XML (`<name>`, `</name>`, `<name attr="val">`, self-closing tags)
//! and copies `tx<nu<...`/`tx<ut<...` text payloads through verbatim.
//! Every *other* line shape -- `mi<mk<...` markers,
//! `ob<nu<open-brack`/`cb<nu<clos-brack` bracket bookkeeping, anything
//! not one of the 7 recognized tag shapes -- is silently dropped: this
//! module's dispatch has no catch-all passthrough arm, unlike every
//! other pass in this crate's rtf2xml port. That's deliberate here --
//! this is the one pass whose whole job is to discard internal
//! pipeline scaffolding and emit only real XML.
//!
//! # The encoding declaration: a representational simplification, not a bug fix
//!
//! Python's `__write_dec` reopens the file being converted as raw
//! bytes and probes it with [`super::check_encoding`] under `us-ascii`,
//! then (if that fails) under a caller-supplied codepage label, falling
//! back to a "bad encoding" warning if *neither* decodes cleanly. This
//! port's `content: &str` is a Rust string -- valid UTF-8 by the
//! language's own type invariant, which no [`super::check_encoding`]
//! probe against an arbitrary encoding label could ever contradict.
//! So: the ASCII check ([`str::is_ascii`], the direct equivalent of the
//! `us-ascii` probe) still meaningfully distinguishes "pure ASCII" from
//! "needs UTF-8", but the *second* probe (Python's `self.__encoding`,
//! e.g. `cp1252`) and the "bad encoding, fall back anyway" branch are
//! provably unreachable here -- content that's already valid UTF-8 is
//! always encodable as UTF-8, full stop. [`convert_to_tags`] therefore
//! takes no `encoding` parameter at all and never produces Python's
//! `bad_encoding` outcome. This is a consequence of this crate's
//! `&str`-based pipeline shape, not a claim that Python's own
//! bad-encoding fallback was wrong.
//!
//! # A crash averted, one preserved
//!
//! `__open_att_func`'s attribute-parsing loop wraps the risky
//! `groups[0]`/`groups[1]` indexing in a broad `except Exception:`,
//! silently skipping a malformed attribute token below `run_level > 3`
//! and raising above it -- ported as [`ConvertToTagsError::IndexOutOfRange`],
//! gated the same way. `__empty_att_func`'s *identical*-looking parsing
//! loop has no such guard at all in Python -- a malformed token there
//! raises an uncaught `IndexError` unconditionally. This port degrades
//! gracefully (silently skips) there too instead of panicking, per this
//! crate's standing convention (see [`super::check_encoding`]'s own doc)
//! of never introducing a panic equivalent to an upstream crash in a
//! pure transform function.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files.

use thiserror::Error;

const PUBLIC_DTD: &str = "rtf2xml1.0.dtd";

const BLOCK: [&str; 43] = [
    "doc",
    "preamble",
    "rtf-definition",
    "font-table",
    "font-in-table",
    "color-table",
    "color-in-table",
    "style-sheet",
    "paragraph-styles",
    "paragraph-style-in-table",
    "character-styles",
    "character-style-in-table",
    "list-table",
    "doc-information",
    "title",
    "author",
    "operator",
    "creation-time",
    "revision-time",
    "editing-time",
    "time",
    "number-of-pages",
    "number-of-words",
    "number-of-characters",
    "page-definition",
    "section-definition",
    "headers-and-footers",
    "section",
    "para",
    "body",
    "paragraph-definition",
    "cell",
    "row",
    "table",
    "revision-table",
    "style-group",
    "border-group",
    "styles-in-body",
    "paragraph-style-in-body",
    "list-in-table",
    "level-in-table",
    "override-table",
    "override-list",
];

const TWO_NEW_LINE: [&str; 5] = ["section", "body", "table", "row", "list-table"];

/// Port of `__open_att_func`'s `run_level > 3`-gated
/// `raise self.__bug_handler(msg)`, reached when an
/// `mi<tg<open-att__` line's attribute token has no `>` separator at
/// all.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConvertToTagsError {
    #[error("index out of range\n")]
    IndexOutOfRange,
}

pub type Result<T> = std::result::Result<T, ConvertToTagsError>;

/// Options `ConvertToTags.__init__` takes beyond the content itself.
#[derive(Debug, Clone)]
pub struct ConvertToTagsOptions {
    pub no_dtd: bool,
    /// `None` -> Python's `dtd_path is None`-ish "else" branch (write
    /// the default public DTD reference). `Some("")` -> Python's
    /// `dtd_path == ''` branch (skip the `<!DOCTYPE>` entirely --
    /// used when further transformations are still going to run).
    /// `Some(path)` -> a custom `SYSTEM` DTD path.
    pub dtd_path: Option<String>,
    pub indent: bool,
    pub run_level: u32,
}

fn token_info(line: &str) -> &str {
    if line.len() >= 16 { &line[..16] } else { line }
}

fn payload(line: &str) -> &str {
    if line.len() >= 17 { &line[17..] } else { "" }
}

fn escape_attr_value(v: &str) -> String {
    v.replace('"', "&quot;").replace('\'', "&quot;")
}

fn write_new_line(out: &mut String, new_line: &mut u32, indent: bool) {
    if !indent {
        return;
    }
    if *new_line == 0 {
        out.push('\n');
        *new_line += 1;
    }
}

fn write_extra_new_line(out: &mut String, new_line: u32, indent: bool) {
    if !indent {
        return;
    }
    if new_line < 2 {
        out.push('\n');
    }
}

/// Port of `__open_func`. Unlike every other tag-writer in this file,
/// this one does its newline bookkeeping *before* writing the tag.
fn open_tag(out: &mut String, new_line: &mut u32, line: &str, indent: bool) {
    let info = payload(line);
    *new_line = 0;
    if BLOCK.contains(&info) {
        write_new_line(out, new_line, indent);
    }
    if TWO_NEW_LINE.contains(&info) {
        write_extra_new_line(out, *new_line, indent);
    }
    out.push('<');
    out.push_str(info);
    out.push('>');
}

/// Port of `__empty_func`.
fn empty_tag(out: &mut String, new_line: &mut u32, line: &str, indent: bool) {
    let info = payload(line);
    out.push('<');
    out.push_str(info);
    out.push_str("/>");
    *new_line = 0;
    if BLOCK.contains(&info) {
        write_new_line(out, new_line, indent);
    }
    if TWO_NEW_LINE.contains(&info) {
        write_extra_new_line(out, *new_line, indent);
    }
}

/// Port of `__close_func`.
fn close_tag(out: &mut String, new_line: &mut u32, line: &str, indent: bool) {
    let info = payload(line);
    out.push_str("</");
    out.push_str(info);
    out.push('>');
    *new_line = 0;
    if BLOCK.contains(&info) {
        write_new_line(out, new_line, indent);
    }
    if TWO_NEW_LINE.contains(&info) {
        write_extra_new_line(out, *new_line, indent);
    }
}

/// Port of `__open_att_func`. `info.split('<')`'s first piece is the
/// element name; each remaining piece splits on `>` into (attribute
/// name, attribute value) -- Python's own `token.split('>')` (no
/// maxsplit) plus `groups[0]`/`groups[1]` indexing means only the
/// first two `>`-delimited pieces are ever used; anything after a
/// second `>` in one attribute token is silently dropped, preserved
/// here via the same `groups.get(0)`/`groups.get(1)` shape.
fn open_att_tag(out: &mut String, new_line: &mut u32, line: &str, indent: bool, run_level: u32) -> Result<()> {
    let info = payload(line);
    let mut parts = info.split('<');
    let element_name = parts.next().unwrap_or("");
    out.push('<');
    out.push_str(element_name);
    for token in parts {
        let groups: Vec<&str> = token.split('>').collect();
        match (groups.first(), groups.get(1)) {
            (Some(val), Some(att)) => {
                let att = escape_attr_value(att);
                out.push_str(&format!(" {val}=\"{att}\""));
            }
            _ => {
                if run_level > 3 {
                    return Err(ConvertToTagsError::IndexOutOfRange);
                }
            }
        }
    }
    out.push('>');
    *new_line = 0;
    if BLOCK.contains(&element_name) {
        write_new_line(out, new_line, indent);
    }
    if TWO_NEW_LINE.contains(&element_name) {
        write_extra_new_line(out, *new_line, indent);
    }
    Ok(())
}

/// Port of `__empty_att_func` -- see this module's own doc for why a
/// malformed attribute token is silently skipped here rather than
/// panicking, unlike [`open_att_tag`]'s explicit `run_level`-gated
/// error (Python itself has no such gate on this one at all).
fn empty_att_tag(out: &mut String, new_line: &mut u32, line: &str, indent: bool) {
    let info = payload(line);
    let mut parts = info.split('<');
    let element_name = parts.next().unwrap_or("");
    out.push('<');
    out.push_str(element_name);
    for token in parts {
        let groups: Vec<&str> = token.split('>').collect();
        if let (Some(val), Some(att)) = (groups.first(), groups.get(1)) {
            let att = escape_attr_value(att);
            out.push_str(&format!(" {val}=\"{att}\""));
        }
    }
    out.push_str("/>");
    *new_line = 0;
    if BLOCK.contains(&element_name) {
        write_new_line(out, new_line, indent);
    }
    if TWO_NEW_LINE.contains(&element_name) {
        write_extra_new_line(out, *new_line, indent);
    }
}

/// Port of `__text_func`.
fn write_text(out: &mut String, line: &str) {
    out.push_str(payload(line));
}

/// Port of `__write_dec`. See this module's own doc for why there's
/// no `encoding` parameter and no "bad encoding" outcome here.
fn write_declaration(out: &mut String, new_line: &mut u32, content: &str, opts: &ConvertToTagsOptions) {
    if content.is_ascii() {
        out.push_str("<?xml version=\"1.0\" encoding=\"US-ASCII\" ?>");
    } else {
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>");
    }
    *new_line = 0;
    write_new_line(out, new_line, opts.indent);

    if opts.no_dtd {
        // Python: `if self.__no_dtd: pass`.
    } else if let Some(path) = &opts.dtd_path {
        if !path.is_empty() {
            out.push_str(&format!("<!DOCTYPE doc SYSTEM \"{path}\">"));
        }
        // Python: `elif self.__dtd_path == '': pass`.
    } else {
        out.push_str(&format!(
            "<!DOCTYPE doc PUBLIC \"publicID\" \"http://rtf2xml.sourceforge.net/dtd/{PUBLIC_DTD}\">"
        ));
    }
    *new_line = 0;
    write_new_line(out, new_line, opts.indent);
}

/// Port of `ConvertToTags.convert_to_tags`, operating directly on
/// intermediate-format content (see this module's own doc) rather
/// than reopening a file.
pub fn convert_to_tags(content: &str, opts: &ConvertToTagsOptions) -> Result<String> {
    let mut out = String::new();
    let mut new_line = 0u32;

    write_declaration(&mut out, &mut new_line, content, opts);

    for line in content.lines() {
        let tok = token_info(line);
        match tok {
            "mi<tg<open______" => open_tag(&mut out, &mut new_line, line, opts.indent),
            "mi<tg<close_____" => close_tag(&mut out, &mut new_line, line, opts.indent),
            "mi<tg<open-att__" => open_att_tag(&mut out, &mut new_line, line, opts.indent, opts.run_level)?,
            "mi<tg<empty-att_" => empty_att_tag(&mut out, &mut new_line, line, opts.indent),
            "tx<nu<__________" | "tx<ut<__________" => write_text(&mut out, line),
            "mi<tg<empty_____" => empty_tag(&mut out, &mut new_line, line, opts.indent),
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> ConvertToTagsOptions {
        ConvertToTagsOptions { no_dtd: true, dtd_path: None, indent: false, run_level: 1 }
    }

    fn strip_decl(out: &str) -> &str {
        // Every test below sets no_dtd: true, so the declaration is
        // always exactly one fixed-length prefix.
        &out[out.find("?>").map(|i| i + 2).unwrap_or(0)..]
    }

    #[test]
    fn open_and_close_tags_convert_to_real_xml_tags() {
        let content = "mi<tg<open______<body\ntx<nu<__________<hi\nmi<tg<close_____<body\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "<body>hi</body>");
    }

    #[test]
    fn unrecognized_line_shapes_are_dropped_not_passed_through() {
        let content = "mi<mk<some-marker\nob<nu<open-brack<0001\ntx<nu<__________<hi\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "hi");
    }

    #[test]
    fn open_att_writes_escaped_quoted_attributes() {
        let content = "mi<tg<open-att__<cell<width>3\"<name>it's ok\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "<cell width=\"3&quot;\" name=\"it&quot;s ok\">");
    }

    #[test]
    fn empty_att_writes_a_self_closing_tag_with_attributes() {
        let content = "mi<tg<empty-att_<cell<width>100\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "<cell width=\"100\"/>");
    }

    #[test]
    fn empty_tag_writes_a_bare_self_closing_tag() {
        let content = "mi<tg<empty_____<br\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "<br/>");
    }

    #[test]
    fn a_malformed_open_att_token_is_skipped_at_low_run_level_and_errors_at_high() {
        let content = "mi<tg<open-att__<cell<novalue\n";
        let mut low = opts();
        low.run_level = 1;
        assert_eq!(strip_decl(&convert_to_tags(content, &low).unwrap()), "<cell>");

        let mut high = opts();
        high.run_level = 4;
        assert_eq!(convert_to_tags(content, &high).unwrap_err(), ConvertToTagsError::IndexOutOfRange);
    }

    #[test]
    fn a_malformed_empty_att_token_is_silently_skipped_rather_than_panicking() {
        let content = "mi<tg<empty-att_<cell<novalue\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert_eq!(strip_decl(&out), "<cell/>");
    }

    #[test]
    fn block_and_two_new_line_tags_get_newlines_only_when_indent_is_set() {
        let content = "mi<tg<open______<body\nmi<tg<close_____<body\n";
        let out = convert_to_tags(content, &opts()).unwrap();
        assert!(!strip_decl(&out).contains('\n'), "{out}");

        let mut indented = opts();
        indented.indent = true;
        let out2 = convert_to_tags(content, &indented).unwrap();
        let out2 = strip_decl(&out2);
        // `body` is both a BLOCK and a TWO_NEW_LINE element.
        // open_tag does its newline bookkeeping *before* writing the
        // tag; close_tag does it *after* -- a real, distinct ordering
        // between the two, not just "newlines appear somewhere".
        assert!(out2.contains("\n<body>"), "{out2}");
        assert!(out2.contains("</body>\n"), "{out2}");
    }

    #[test]
    fn the_declaration_switches_between_us_ascii_and_utf8_by_content() {
        let ascii = convert_to_tags("tx<nu<__________<hello\n", &opts()).unwrap();
        assert!(ascii.starts_with("<?xml version=\"1.0\" encoding=\"US-ASCII\" ?>"), "{ascii}");

        let non_ascii = convert_to_tags("tx<nu<__________<caf\u{e9}\n", &opts()).unwrap();
        assert!(non_ascii.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>"), "{non_ascii}");
    }

    #[test]
    fn dtd_path_variants_match_python_branches() {
        let content = "tx<nu<__________<x\n";

        let mut no_dtd = opts();
        no_dtd.no_dtd = true;
        assert!(!convert_to_tags(content, &no_dtd).unwrap().contains("DOCTYPE"));

        let mut custom = opts();
        custom.no_dtd = false;
        custom.dtd_path = Some("my.dtd".to_string());
        assert!(convert_to_tags(content, &custom).unwrap().contains("<!DOCTYPE doc SYSTEM \"my.dtd\">"));

        let mut empty = opts();
        empty.no_dtd = false;
        empty.dtd_path = Some(String::new());
        assert!(!convert_to_tags(content, &empty).unwrap().contains("DOCTYPE"));

        let mut default = opts();
        default.no_dtd = false;
        default.dtd_path = None;
        let out = convert_to_tags(content, &default).unwrap();
        assert!(out.contains("http://rtf2xml.sourceforge.net/dtd/rtf2xml1.0.dtd"), "{out}");
    }
}
