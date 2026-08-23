//! Port of `old_src/src/calibre/ebooks/docx/fields.py` -- **the pure
//! field-instruction parsing half only** (issue #290): the [`Field`]
//! accumulator and the `\flag "quoted word" bareword`-syntax
//! [`scan`]ner plus its five named parsers ([`parse_hyperlink`]/
//! [`parse_xe`]/[`parse_index`]/[`parse_ref`]/[`parse_noteref`]).
//!
//! The `Fields` orchestrator itself (`__call__`'s stack-based walk
//! over the source document collecting `w:fldChar`/`w:fldSimple`
//! field boundaries into [`Field`]s, `get_runs`, and the *method*-level
//! `parse_hyperlink`/`parse_ref`/`parse_xe`/`parse_index`/
//! `polish_markup` -- same names as this file's module-level parsers,
//! but a different, field-*dispatching* role) is a separate, larger
//! follow-up. Two real reasons, not just size:
//!
//! - `parse_xe` inserts a synthetic `w:bookmarkStart`/`w:bookmarkEnd`
//!   pair into the *source* document tree so a later pass can link to
//!   an index entry -- exactly the source-tree-mutation need
//!   `crate::xmltree` exists for (see `docx/mod.rs`'s module docs),
//!   but this port's `to_html.rs` pipeline is built entirely on
//!   `roxmltree`'s read-only tree. Whether that's solved by finally
//!   using `crate::xmltree` here, or (like `tables.rs`'s
//!   `removed_cells`) by tracking the synthetic bookmark as side-table
//!   state `convert_p`'s existing `w:bookmarkStart` handling can
//!   consult, is an open design question for that follow-up.
//! - `parse_index` and `polish_markup` call straight into `index.py`
//!   (`process_index`/`polish_index_markup`), which is **not yet
//!   ported** (issue #293) -- a real forward dependency #290's own
//!   issue body didn't mention. `Fields.__call__` handles every field
//!   type in one pass, so it can't be split around this the way
//!   `to_html.rs`'s many independent functions have been.
//!
//! `parser(...)`'s returned closure has an unused `log` parameter,
//! dropped here since the closure body never references it.

use std::collections::HashMap;

use roxmltree::Node;

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
}
