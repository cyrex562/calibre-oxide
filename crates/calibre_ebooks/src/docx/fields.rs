//! Port of `old_src/src/calibre/ebooks/docx/fields.py` (issue #290):
//! the pure field-instruction parsing half ([`Field`], the
//! `\flag "quoted word" bareword`-syntax [`scan`]ner, and its five
//! named parsers -- [`parse_hyperlink`]/[`parse_xe`]/[`parse_index`]/
//! [`parse_ref`]/[`parse_noteref`]), plus [`FieldsCollector`]: the
//! source-tree-only half of the `Fields` orchestrator (`__call__`'s
//! stack-based walk collecting `w:fldChar`/`w:fldSimple` field
//! boundaries into [`Field`]s, dispatching each by name to the
//! parsers above).
//!
//! [`FieldsCollector::collect`] runs before the main body walk (same
//! as Python's own `self.fields(doc, self.log)`, called right after
//! `resolve_alternate_content`), since it only reads the source tree.
//! What it *doesn't* do, unlike Python's single-pass `Fields.__call__`,
//! is call `docx/index.rs`'s `process_index` inline or assign an `XE`
//! field's anchor id -- both need the HTML tree/`ConvertState::object_map`
//! that only exists *after* the main body walk. This was the open
//! architectural question issue #290 was tracked as blocked on
//! (real `crate::xmltree` source-tree mutation vs. a side-table) --
//! resolved the same way `docx/index.rs`'s own `process_index` (issue
//! #293, closed) resolved it: the synthetic bookmark's only real
//! purpose is giving a *name* to `field.start`'s (a real,
//! already-parsed node's) eventual HTML position, so a later pass,
//! not yet written, can just look up whichever HTML element
//! `field.start`'s enclosing `w:r` became (via `object_map`'s reverse
//! lookup -- the same technique `to_html.rs`'s own `resolve_links`
//! already uses for its own deferred `Fields.hyperlink_fields` block)
//! and stamp an `id` there directly -- no real tree mutation needed.
//!
//! Still needed, none of it blocked on any open design question
//! anymore: a post-body-walk pass assigning each `XeFieldData`'s
//! anchor id (with the same document-wide uniqueness check Python's
//! own `index_bookmark_prefix` loop makes, checked against the HTML
//! ids already in use rather than a source-tree `@w:id` scan) and
//! stamping it; calling `process_index` for each collected `INDEX`
//! field once every `XE` field's anchor is assigned (a genuine,
//! disclosed divergence from Python here: since this happens *after*
//! the whole document is walked rather than inline during a single
//! pass, an `INDEX` field here sees every `XE` field in the document,
//! not just the ones Python's own single pass had already dispatched
//! by the time it reached that `INDEX` field -- see
//! [`FieldsCollector`]'s own docs); and resolving
//! `FieldsCollector::hyperlink_fields` into real `<a>` elements,
//! extending `to_html.rs`'s `resolve_links` with the block it was
//! always missing for this input source specifically.
//!
//! `parser(...)`'s returned closure has an unused `log` parameter,
//! dropped here since the closure body never references it.

use std::collections::HashMap;

use roxmltree::Node;

use super::names::DocxNamespace;

/// One in-progress or finished field, spanning from a `w:fldChar`
/// begin (or a `w:fldSimple`) to its matching end. Port of `Field`.
#[derive(Debug, Clone)]
pub struct Field<'a, 'i> {
    pub start: Node<'a, 'i>,
    pub end: Option<Node<'a, 'i>>,
    pub contents: Vec<Node<'a, 'i>>,
    buf: Vec<String>,
    pub instructions: Option<String>,
    pub name: Option<String>,
}

impl<'a, 'i> Field<'a, 'i> {
    pub fn new(start: Node<'a, 'i>) -> Self {
        Field {
            start,
            end: None,
            contents: Vec::new(),
            buf: Vec::new(),
            instructions: None,
            name: None,
        }
    }

    /// Port of `Field.add_instr`.
    pub fn add_instr(&mut self, elem: Node<'a, 'i>) {
        self.add_raw(elem.text().unwrap_or(""));
    }

    /// Port of `Field.add_raw`. The field's `name` is the first
    /// whitespace-trimmed word of the first non-empty instruction
    /// fragment seen; everything after that first space (on that
    /// first fragment only) becomes the start of `instructions`.
    pub fn add_raw(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        let mut raw = raw.to_string();
        if self.name.is_none() {
            let trimmed = raw.trim_start();
            let (name, rest) = match trimmed.split_once(' ') {
                Some((n, r)) => (n.to_string(), r.to_string()),
                None => (trimmed.to_string(), String::new()),
            };
            self.name = Some(name);
            raw = rest;
        }
        self.buf.push(raw);
    }

    /// Port of `Field.finalize`.
    pub fn finalize(&mut self) {
        self.instructions = Some(self.buf.join(""));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Word,
    Flag,
}

/// Port of the module-level `scanner = re.Scanner([...])`. Tries, at
/// each position: a backslash-flag (`\` + exactly one non-whitespace
/// char), a `"quoted word"`, a bare non-whitespace word (not starting
/// with `\`/`"`), or whitespace (skipped, no token) -- in that order.
/// If none match (an unterminated quote, or a trailing lone `\` with
/// nothing/whitespace after it), scanning stops there and everything
/// from that point on is silently dropped, matching `re.Scanner.scan`
/// itself (its unconsumed-remainder return value is never used by
/// `parser`'s `parse`, which only keeps the token list).
fn scan(raw: &str) -> Vec<(String, TokenKind)> {
    let mut tokens = Vec::new();
    let mut rest = raw;
    loop {
        let Some(c0) = rest.chars().next() else {
            break;
        };

        if c0.is_whitespace() {
            let end = rest
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }

        if c0 == '\\' {
            let mut chars = rest.char_indices();
            chars.next();
            match chars.next() {
                Some((i1, c1)) if !c1.is_whitespace() => {
                    let end = i1 + c1.len_utf8();
                    tokens.push((rest[..end].to_string(), TokenKind::Flag));
                    rest = &rest[end..];
                }
                _ => break,
            }
            continue;
        }

        if c0 == '"' {
            match rest[1..].find('"') {
                Some(rel_end) => {
                    let inner = &rest[1..1 + rel_end];
                    tokens.push((inner.to_string(), TokenKind::Word));
                    rest = &rest[1 + rel_end + 1..];
                }
                None => break,
            }
            continue;
        }

        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        tokens.push((rest[..end].to_string(), TokenKind::Word));
        rest = &rest[end..];
    }
    tokens
}

/// A parsed field instruction: named-flag values plus (for parsers
/// with no `default_field_name`, i.e. `index`/`ref`/`noteref`) a
/// `None`-keyed positional value. Port of `parse`'s return `dict`.
pub type FieldValues = HashMap<Option<String>, Option<String>>;

/// Port of `parser(...)`'s returned closure. `field_map` is `(flag
/// char, option name)` pairs (Python's `'l:anchor m:image-map ...'`,
/// pre-split); `default_field_name` is the option a flagless leading
/// word is stored under (`None` for `index`/`ref`/`noteref`, which
/// have none in Python either).
fn parse_instructions(
    raw: &str,
    field_map: &[(char, &str)],
    default_field_name: Option<&str>,
) -> FieldValues {
    enum LastOption {
        None,
        Known(String),
        Unknown,
    }

    let mut ans: FieldValues = HashMap::new();
    let mut last_option = LastOption::None;

    let masked = raw.replace("\\\\", "\u{1}").replace("\\\"", "\u{2}");
    for (token, kind) in scan(&masked) {
        let token = token.replace('\u{1}', "\\").replace('\u{2}', "\"");
        match kind {
            TokenKind::Flag => {
                let flag_char = token.chars().nth(1);
                let mapped = flag_char
                    .and_then(|c| field_map.iter().find(|pair| pair.0 == c))
                    .map(|pair| pair.1);
                last_option = match mapped {
                    Some(name) => {
                        ans.insert(Some(name.to_string()), None);
                        LastOption::Known(name.to_string())
                    }
                    None => LastOption::Unknown,
                };
            }
            TokenKind::Word => match &last_option {
                LastOption::None => {
                    ans.insert(default_field_name.map(str::to_string), Some(token));
                }
                LastOption::Known(name) => {
                    ans.insert(Some(name.clone()), Some(token));
                    last_option = LastOption::None;
                }
                LastOption::Unknown => {
                    last_option = LastOption::None;
                }
            },
        }
    }
    ans
}

const HYPERLINK_FIELDS: &[(char, &str)] = &[
    ('l', "anchor"),
    ('m', "image-map"),
    ('n', "target"),
    ('o', "title"),
    ('t', "target"),
];

const XE_FIELDS: &[(char, &str)] = &[
    ('b', "bold"),
    ('i', "italic"),
    ('f', "entry-type"),
    ('r', "page-range-bookmark"),
    ('t', "page-number-text"),
    ('y', "yomi"),
];

const INDEX_FIELDS: &[(char, &str)] = &[
    ('b', "bookmark"),
    ('c', "columns-per-page"),
    ('d', "sequence-separator"),
    ('e', "first-page-number-separator"),
    ('f', "entry-type"),
    ('g', "page-range-separator"),
    ('h', "heading"),
    ('k', "crossref-separator"),
    ('l', "page-number-separator"),
    ('p', "letter-range"),
    ('s', "sequence-name"),
    ('r', "run-together"),
    ('y', "yomi"),
    ('z', "langcode"),
];

const REF_FIELDS: &[(char, &str)] = &[
    ('d', "separator"),
    ('f', "footnote"),
    ('h', "hyperlink"),
    ('n', "number"),
    ('p', "position"),
    ('r', "relative-number"),
    ('t', "suppress"),
    ('w', "number-full-context"),
];

const NOTEREF_FIELDS: &[(char, &str)] = &[('f', "footnote"), ('h', "hyperlink"), ('p', "position")];

/// Port of `parse_hyperlink = parser('hyperlink', ..., 'url')`.
pub fn parse_hyperlink(raw: &str) -> FieldValues {
    parse_instructions(raw, HYPERLINK_FIELDS, Some("url"))
}

/// Port of `parse_xe = parser('xe', ..., 'text')`.
pub fn parse_xe(raw: &str) -> FieldValues {
    parse_instructions(raw, XE_FIELDS, Some("text"))
}

/// Port of `parse_index = parser('index', ...)` (no default field).
pub fn parse_index(raw: &str) -> FieldValues {
    parse_instructions(raw, INDEX_FIELDS, None)
}

/// Port of `parse_ref = parser('ref', ...)` (no default field).
pub fn parse_ref(raw: &str) -> FieldValues {
    parse_instructions(raw, REF_FIELDS, None)
}

/// Port of `parse_noteref = parser('noteref', ...)` (no default field).
pub fn parse_noteref(raw: &str) -> FieldValues {
    parse_instructions(raw, NOTEREF_FIELDS, None)
}

/// One `XE` field's data, collected by [`FieldsCollector::collect`]
/// but not yet given an anchor id -- that assignment needs the HTML
/// tree, deferred to a later pass (see [`FieldsCollector`]'s own
/// docs for why).
///
/// Port of the `xe` dict `Fields.parse_xe` builds, minus `anchor`
/// (assigned later) and `start_elem` (this struct's own `start`
/// already is that node).
#[derive(Debug, Clone)]
pub struct XeFieldData<'a, 'i> {
    pub text: String,
    pub entry_type: Option<String>,
    pub page_number_text: Option<String>,
    pub start: Node<'a, 'i>,
    pub end: Node<'a, 'i>,
}

/// Everything one pass over the source document gathers, before the
/// main body walk runs.
///
/// Port of `Fields.__call__`'s field-collection-and-dispatch loop --
/// with two real, disclosed differences from it, both forced by the
/// same fact: `process_index` (`docx/index.rs`, issue #293) and
/// stamping an `XE` field's anchor `id` both need the HTML tree
/// (`crate::dom::Dom`/`ConvertState::object_map`) that only exists
/// *after* the main body walk -- but Python's own `self.fields(doc,
/// self.log)` runs *before* it (`Convert.__call__` calls it right
/// after `resolve_alternate_content`, well before
/// `read_page_properties`), and calls `process_index` for each
/// `INDEX` field *inline*, during this same single pass:
///
/// - `xe_fields` here carries no `anchor` yet -- a later pass (not
///   yet ported) assigns one per field, once the HTML tree exists, by
///   finding whichever HTML element `start`'s enclosing `w:r` became
///   (via `ConvertState::object_map`'s reverse lookup, the same
///   technique `to_html.rs`'s own `resolve_links` already uses for
///   its `hyperlink_fields` block) and stamping an `id` there.
/// - `index_fields` holds each `INDEX` field's own parsed switches
///   plus its `field.contents` -- not yet resolved into generated
///   HTML blocks. Calling `process_index` for real needs to happen
///   after that later pass, so every `XE` field in the *whole*
///   document is available by then, not just the ones this walk had
///   already dispatched by the time it reached a given `INDEX` field.
///   This is a genuine, disclosed behavior change from Python's own
///   single-pass architecture, where an `INDEX` field only ever sees
///   `XE` fields that appear *before* it in document order (an
///   accident of *when* `self.xe_fields` happens to have been
///   populated, not a deliberate design choice) -- not reproduced,
///   since there is nothing to reproduce a document-order accident
///   *for*.
///
/// Port of `Fields.get_runs` sits alongside this as a free function
/// ([`get_runs`]), since every dispatcher needs it and none of them
/// are methods on anything stateful enough to own it.
#[derive(Debug, Default)]
pub struct FieldsCollector<'a, 'i> {
    pub hyperlink_fields: Vec<(FieldValues, Vec<Node<'a, 'i>>)>,
    pub xe_fields: Vec<XeFieldData<'a, 'i>>,
    pub index_fields: Vec<(super::index::IndexField, Vec<Node<'a, 'i>>)>,
    /// Field names encountered that aren't `HYPERLINK`/`hyperlink`,
    /// `XE`/`xe`, `INDEX`/`index`, `REF`/`ref`, `NOTEREF`/`noteref`,
    /// `TOC`/`toc`, or `PAGEREF`/`pageref` (`TOC`/`PAGEREF` are
    /// handled elsewhere already -- `toc.py`'s own port for the
    /// former, nothing at all for the latter, matching Python).
    ///
    /// Port of `log.warn(f'Encountered unknown field: {field.name},
    /// ignoring it.')`, tracked as data instead of a log call -- no
    /// logger threads through this module, same as every other
    /// function in this crate that silently drops what Python would
    /// have logged. Unlike Python's own `unknown_fields` set (which
    /// exists purely to avoid warning about the *same* unknown field
    /// name twice), this keeps every occurrence -- deduplication was
    /// a log-spam concern, not something a data-collecting caller
    /// needs.
    pub unknown_fields: Vec<String>,
}

impl<'a, 'i> FieldsCollector<'a, 'i> {
    /// Port of `Fields.__call__`. See [`FieldsCollector`]'s own docs
    /// for what's deferred and why.
    pub fn collect(document: Node<'a, 'i>, ns: &DocxNamespace) -> Self {
        let mut fields: Vec<Field<'a, 'i>> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();

        for elem in ns.descendants(
            document,
            &["w:p", "w:r", "w:instrText", "w:fldChar", "w:fldSimple"],
        ) {
            if ns.is_tag(elem, "w:fldChar") {
                match ns.get(elem, "w:fldCharType") {
                    Some("begin") => {
                        fields.push(Field::new(elem));
                        stack.push(fields.len() - 1);
                    }
                    Some("end") => {
                        if let Some(idx) = stack.pop() {
                            fields[idx].end = Some(elem);
                        }
                    }
                    _ => {}
                }
            } else if ns.is_tag(elem, "w:instrText") {
                if let Some(&idx) = stack.last() {
                    fields[idx].add_instr(elem);
                }
            } else if ns.is_tag(elem, "w:fldSimple") {
                if let Some(instr) = ns.get(elem, "w:instr").filter(|s| !s.is_empty()) {
                    let mut field = Field::new(elem);
                    field.add_raw(instr);
                    for r in ns.descendants(elem, &["w:r"]) {
                        field.contents.push(r);
                    }
                    fields.push(field);
                }
            } else if let Some(&idx) = stack.last() {
                // A `w:p`/`w:r` encountered while a field is open.
                fields[idx].contents.push(elem);
            }
        }

        let mut result = FieldsCollector::default();
        for mut field in fields {
            field.finalize();
            let Some(instructions) = field.instructions.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            let instructions = instructions.to_string();
            let name = field.name.clone().unwrap_or_default();
            match name.as_str() {
                "HYPERLINK" | "hyperlink" => {
                    dispatch_hyperlink(&mut result, &field, &instructions, ns)
                }
                "REF" | "ref" => dispatch_ref(&mut result, &field, &instructions, parse_ref, ns),
                "NOTEREF" | "noteref" => {
                    dispatch_ref(&mut result, &field, &instructions, parse_noteref, ns)
                }
                "XE" | "xe" => dispatch_xe(&mut result, &field, &instructions),
                "INDEX" | "index" => dispatch_index(&mut result, &field, &instructions),
                "TOC" | "toc" | "PAGEREF" | "pageref" => {}
                other => result.unknown_fields.push(other.to_string()),
            }
        }
        result
    }
}

/// Splits `contents` (a field's `Field::contents`) into groups of
/// consecutive `w:r` elements, one group per `w:p` boundary crossed
/// -- "we only handle spans in a single paragraph being wrapped in
/// `<a>`" (Python's own comment).
///
/// Port of `Fields.get_runs`.
fn get_runs<'a, 'i>(contents: &[Node<'a, 'i>], ns: &DocxNamespace) -> Vec<Vec<Node<'a, 'i>>> {
    let mut all_runs = Vec::new();
    let mut current_runs: Vec<Node<'a, 'i>> = Vec::new();
    for &x in contents {
        if ns.is_tag(x, "w:p") {
            if !current_runs.is_empty() {
                all_runs.push(std::mem::take(&mut current_runs));
            }
        } else if ns.is_tag(x, "w:r") {
            current_runs.push(x);
        }
    }
    if !current_runs.is_empty() {
        all_runs.push(current_runs);
    }
    all_runs
}

/// Port of `Fields.parse_hyperlink`.
fn dispatch_hyperlink<'a, 'i>(
    result: &mut FieldsCollector<'a, 'i>,
    field: &Field<'a, 'i>,
    instructions: &str,
    ns: &DocxNamespace,
) {
    let mut hl = parse_hyperlink(instructions);
    if hl.is_empty() {
        return;
    }
    let target_key = Some("target".to_string());
    if hl.get(&target_key) == Some(&None) {
        hl.insert(target_key, Some("_blank".to_string()));
    }
    for runs in get_runs(&field.contents, ns) {
        result.hyperlink_fields.push((hl.clone(), runs));
    }
}

/// Port of `Fields.parse_ref` (also `Fields.parse_noteref`, an alias
/// for the same method in Python -- `parse` is `parse_ref`/
/// `parse_noteref` respectively at the two call sites).
fn dispatch_ref<'a, 'i>(
    result: &mut FieldsCollector<'a, 'i>,
    field: &Field<'a, 'i>,
    instructions: &str,
    parse: fn(&str) -> FieldValues,
    ns: &DocxNamespace,
) {
    let r = parse(instructions);
    let dest = r.get(&None).cloned().flatten();
    let has_hyperlink_flag = r.contains_key(&Some("hyperlink".to_string()));
    let Some(dest) = dest.filter(|_| has_hyperlink_flag) else {
        return; // log.warn(...), dropped -- see FieldsCollector's own docs.
    };
    let mut hl = FieldValues::new();
    hl.insert(Some("anchor".to_string()), Some(dest));
    for runs in get_runs(&field.contents, ns) {
        result.hyperlink_fields.push((hl.clone(), runs));
    }
}

/// Port of `Fields.parse_xe`, minus the synthetic bookmark insertion
/// -- see [`FieldsCollector`]'s own docs for why that's deferred to a
/// later pass instead.
fn dispatch_xe<'a, 'i>(
    result: &mut FieldsCollector<'a, 'i>,
    field: &Field<'a, 'i>,
    instructions: &str,
) {
    let Some(end) = field.end else { return };
    let xe = parse_xe(instructions);
    if xe.is_empty() {
        return;
    }
    result.xe_fields.push(XeFieldData {
        text: xe
            .get(&Some("text".to_string()))
            .cloned()
            .flatten()
            .unwrap_or_default(),
        entry_type: xe.get(&Some("entry-type".to_string())).cloned().flatten(),
        page_number_text: xe
            .get(&Some("page-number-text".to_string()))
            .cloned()
            .flatten(),
        start: field.start,
        end,
    });
}

/// Port of `Fields.parse_index`, minus the `process_index` call
/// itself -- see [`FieldsCollector`]'s own docs for why that's
/// deferred to a later pass instead.
fn dispatch_index<'a, 'i>(
    result: &mut FieldsCollector<'a, 'i>,
    field: &Field<'a, 'i>,
    instructions: &str,
) {
    if field.contents.is_empty() {
        return;
    }
    let idx = parse_index(instructions);
    let index_field = super::index::IndexField {
        heading: idx.get(&Some("heading".to_string())).cloned().flatten(),
        entry_type: idx.get(&Some("entry-type".to_string())).cloned().flatten(),
        letter_range: idx
            .get(&Some("letter-range".to_string()))
            .cloned()
            .flatten(),
        bookmark: idx.get(&Some("bookmark".to_string())).cloned().flatten(),
    };
    result
        .index_fields
        .push((index_field, field.contents.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    /// Port of `TestParseFields.test_hyperlink`.
    #[test]
    fn hyperlink_field_instructions() {
        assert_eq!(
            parse_hyperlink(r"\l anchor1"),
            HashMap::from([(some("anchor"), some("anchor1"))])
        );
        assert_eq!(
            parse_hyperlink("www.calibre-ebook.com"),
            HashMap::from([(some("url"), some("www.calibre-ebook.com"))])
        );
        assert_eq!(
            parse_hyperlink(r"www.calibre-ebook.com \t target \o tt"),
            HashMap::from([
                (some("url"), some("www.calibre-ebook.com")),
                (some("target"), some("target")),
                (some("title"), some("tt")),
            ])
        );
        assert_eq!(
            parse_hyperlink(r#""c:\\Some Folder""#),
            HashMap::from([(some("url"), some("c:\\Some Folder"))])
        );
        assert_eq!(
            parse_hyperlink(r"xxxx \y yyyy"),
            HashMap::from([(some("url"), some("xxxx"))])
        );
    }

    /// Port of `TestParseFields.test_xe`.
    #[test]
    fn xe_field_instructions() {
        assert_eq!(
            parse_xe(r#""some name""#),
            HashMap::from([(some("text"), some("some name"))])
        );
        assert_eq!(
            parse_xe(r"name \b \i"),
            HashMap::from([
                (some("text"), some("name")),
                (some("bold"), None),
                (some("italic"), None)
            ])
        );
        assert_eq!(
            parse_xe(r"xxx \y a"),
            HashMap::from([(some("text"), some("xxx")), (some("yomi"), some("a"))])
        );
    }

    /// Port of `TestParseFields.test_index`.
    #[test]
    fn index_field_instructions() {
        assert_eq!(parse_index(""), HashMap::new());
        assert_eq!(
            parse_index(r"\b \c 1"),
            HashMap::from([
                (some("bookmark"), None),
                (some("columns-per-page"), some("1"))
            ])
        );
    }

    #[test]
    fn ref_field_uses_the_none_key_for_the_bare_destination_word() {
        let parsed = parse_ref("chap1 \\h");
        assert_eq!(parsed.get(&None), Some(&some("chap1")));
        assert_eq!(parsed.get(&some("hyperlink")), Some(&None));
    }

    #[test]
    fn an_escaped_backslash_is_not_mistaken_for_a_flag() {
        // `\\` (escaped) followed by `l` is the two-character word
        // `\l`, not a `\l` flag.
        let parsed = parse_hyperlink(r"\\l");
        assert_eq!(parsed.get(&some("url")), Some(&some(r"\l")));
        assert!(!parsed.contains_key(&some("anchor")));
    }

    #[test]
    fn an_escaped_quote_survives_inside_a_quoted_word() {
        let parsed = parse_hyperlink(r#""say \"hi\"""#);
        assert_eq!(parsed.get(&some("url")), Some(&some(r#"say "hi""#)));
    }

    #[test]
    fn an_unterminated_quote_stops_scanning_there() {
        // No closing `"` -- matches `re.Scanner`'s own behavior of
        // stopping at the first unmatched position, dropping the rest.
        assert_eq!(
            parse_hyperlink(r#"ok "unterminated"#),
            HashMap::from([(some("url"), some("ok"))])
        );
    }

    #[test]
    fn field_add_raw_sets_name_from_the_first_word_and_buffers_the_rest() {
        let doc = roxmltree::Document::parse(
            r#"<r xmlns:w="x"><w:instrText> HYPERLINK "http://example.com" </w:instrText></r>"#,
        )
        .unwrap();
        let elem = doc.root_element().children().next().unwrap();
        let mut field = Field::new(doc.root_element());
        field.add_instr(elem);
        assert_eq!(field.name.as_deref(), Some("HYPERLINK"));
        field.finalize();
        assert_eq!(
            field.instructions.as_deref(),
            Some(r#""http://example.com" "#)
        );
    }

    #[test]
    fn field_add_raw_ignores_empty_fragments() {
        let doc = roxmltree::Document::parse("<r/>").unwrap();
        let mut field = Field::new(doc.root_element());
        field.add_raw("");
        assert!(field.name.is_none());
    }

    mod fields_collector_tests {
        use super::*;
        use roxmltree::Document;

        const DOC_OPEN: &str =
            r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

        fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
            let xml: &'static str =
                Box::leak(format!("<w:document {DOC_OPEN}>{body}</w:document>").into_boxed_str());
            (
                Document::parse(xml).expect("valid XML"),
                DocxNamespace::default(),
            )
        }

        /// A `w:fldChar`-based field spanning one paragraph: begin,
        /// `instrText`, separate, a result run, end.
        fn field_xml(instr: &str, result_text: &str) -> String {
            format!(
                r#"<w:p>
                     <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                     <w:r><w:instrText>{instr}</w:instrText></w:r>
                     <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                     <w:r><w:t>{result_text}</w:t></w:r>
                     <w:r><w:fldChar w:fldCharType="end"/></w:r>
                   </w:p>"#
            )
        }

        #[test]
        fn a_hyperlink_field_produces_one_hyperlink_entry_wrapping_every_run() {
            let (doc, ns) = parse_root(&field_xml(r#" HYPERLINK "http://example.com" "#, "click"));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.hyperlink_fields.len(), 1);
            let (hl, runs) = &result.hyperlink_fields[0];
            assert_eq!(hl.get(&some("url")), Some(&some("http://example.com")));
            assert_eq!(runs.len(), 4, "every run seen while the field was open");
        }

        #[test]
        fn a_hyperlink_field_with_a_bare_target_flag_defaults_to_blank() {
            let (doc, ns) = parse_root(&field_xml(
                r#" HYPERLINK "http://example.com" \t "#,
                "click",
            ));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            let (hl, _) = &result.hyperlink_fields[0];
            assert_eq!(hl.get(&some("target")), Some(&some("_blank")));
        }

        #[test]
        fn a_ref_field_with_the_hyperlink_switch_produces_an_anchor_entry() {
            let (doc, ns) = parse_root(&field_xml(r" REF bookmark1 \h ", "text"));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.hyperlink_fields.len(), 1);
            let (hl, _) = &result.hyperlink_fields[0];
            assert_eq!(hl.get(&some("anchor")), Some(&some("bookmark1")));
        }

        #[test]
        fn a_ref_field_without_the_hyperlink_switch_is_dropped() {
            let (doc, ns) = parse_root(&field_xml(r" REF bookmark1 ", "text"));
            let result = FieldsCollector::collect(doc.root_element(), &ns);
            assert!(result.hyperlink_fields.is_empty());
        }

        #[test]
        fn a_noteref_field_behaves_like_ref() {
            let (doc, ns) = parse_root(&field_xml(r" NOTEREF note1 \h ", "1"));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.hyperlink_fields.len(), 1);
            let (hl, _) = &result.hyperlink_fields[0];
            assert_eq!(hl.get(&some("anchor")), Some(&some("note1")));
        }

        #[test]
        fn an_xe_field_is_collected_with_its_start_and_end_nodes() {
            let (doc, ns) = parse_root(&field_xml(r#" XE "Apple" \t "5" "#, ""));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.xe_fields.len(), 1);
            let xe = &result.xe_fields[0];
            assert_eq!(xe.text, "Apple");
            assert_eq!(xe.page_number_text.as_deref(), Some("5"));
            assert!(ns.is_tag(xe.start, "w:fldChar"));
            assert!(ns.is_tag(xe.end, "w:fldChar"));
        }

        #[test]
        fn an_xe_field_with_no_matching_end_is_dropped() {
            // No closing fldChar[end] at all -- field.end stays None.
            let (doc, ns) = parse_root(
                r#"<w:p>
                     <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                     <w:r><w:instrText> XE "Apple" </w:instrText></w:r>
                   </w:p>"#,
            );
            let result = FieldsCollector::collect(doc.root_element(), &ns);
            assert!(result.xe_fields.is_empty());
        }

        #[test]
        fn an_index_field_with_content_is_collected() {
            let (doc, ns) = parse_root(&field_xml(r#" INDEX \h "A" "#, "placeholder"));
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.index_fields.len(), 1);
            let (idx, contents) = &result.index_fields[0];
            assert_eq!(idx.heading.as_deref(), Some("A"));
            assert!(!contents.is_empty());
        }

        #[test]
        fn an_index_field_with_no_contents_is_dropped() {
            let (doc, ns) = parse_root(
                r#"<w:p>
                     <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                     <w:r><w:instrText> INDEX </w:instrText></w:r>
                     <w:r><w:fldChar w:fldCharType="end"/></w:r>
                   </w:p>"#,
            );
            let result = FieldsCollector::collect(doc.root_element(), &ns);
            assert!(result.index_fields.is_empty());
        }

        #[test]
        fn an_unknown_field_is_tracked_but_toc_and_pageref_are_not() {
            let (doc, ns) = parse_root(&format!(
                "{}{}{}",
                field_xml(" FOOBAR arg1 ", "x"),
                field_xml(" TOC \\o \"1-3\" ", "x"),
                field_xml(" PAGEREF x ", "1")
            ));
            let result = FieldsCollector::collect(doc.root_element(), &ns);
            assert_eq!(result.unknown_fields, vec!["FOOBAR".to_string()]);
        }

        #[test]
        fn a_fldsimple_hyperlink_is_collected_too() {
            let (doc, ns) = parse_root(
                r#"<w:p>
                     <w:fldSimple w:instr=" HYPERLINK &quot;http://example.com&quot; ">
                       <w:r><w:t>click</w:t></w:r>
                     </w:fldSimple>
                   </w:p>"#,
            );
            let result = FieldsCollector::collect(doc.root_element(), &ns);

            assert_eq!(result.hyperlink_fields.len(), 1);
            let (hl, runs) = &result.hyperlink_fields[0];
            assert_eq!(hl.get(&some("url")), Some(&some("http://example.com")));
            assert_eq!(runs.len(), 1);
        }

        #[test]
        fn nested_fields_each_produce_their_own_hyperlink_entry() {
            // A REF field nested inside a HYPERLINK field's own
            // display text. Each run seen while multiple fields are
            // open goes to whichever field is *innermost* at that
            // moment (Python's own `stack[-1]`, not every open
            // field) -- both still end up with a well-formed
            // (non-empty) `contents` here since the inner field's
            // begin/end markers each get attributed to whichever
            // field was on top of the stack when that specific run
            // was visited.
            let (doc, ns) = parse_root(
                r#"<w:p>
                     <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                     <w:r><w:instrText> HYPERLINK "http://example.com" </w:instrText></w:r>
                     <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                     <w:r><w:fldChar w:fldCharType="begin"/></w:r>
                     <w:r><w:instrText> REF bookmark1 \h </w:instrText></w:r>
                     <w:r><w:fldChar w:fldCharType="separate"/></w:r>
                     <w:r><w:t>inner</w:t></w:r>
                     <w:r><w:fldChar w:fldCharType="end"/></w:r>
                     <w:r><w:fldChar w:fldCharType="end"/></w:r>
                   </w:p>"#,
            );
            let result = FieldsCollector::collect(doc.root_element(), &ns);
            assert_eq!(result.hyperlink_fields.len(), 2);
        }
    }
}
