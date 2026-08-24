//! Port of `Convert.read_styles` (`old_src/.../docx/to_html.py:294-385`)
//! -- the package-loading step that resolves and reads
//! `numbering.xml`/`styles.xml`/`settings.xml`/`theme1.xml`/
//! `footnotes.xml`/`endnotes.xml`. `fontTable.xml` is not read at all:
//! it needs `fonts.py`'s `Fonts` class (embedded-font extraction),
//! itself blocked on a system font scanner -- see `fonts.rs`'s module
//! docs. `Styles`/`Numbering`'s own font resolution already stops
//! short of that dependency for the same reason.
//!
//! # Split into two phases, unlike Python's one method
//!
//! Every part parses into a `roxmltree::Document` that the
//! eventually-populated `Styles`/`Numbering`/`Footnotes`/`Theme` all
//! borrow `Node`s from -- and those borrows need to live as long as
//! the whole conversion does. A single function can't parse a
//! `Document` in a local variable and also hand back something
//! borrowing from it. So:
//!
//! - [`read_raw_parts`] does the real I/O: resolves each part's
//!   zip-internal name ([`resolve_part_name`], Python's `get_name`)
//!   and reads its raw bytes as UTF-8 text, returning them all as
//!   owned [`RawParts`] -- no lifetime entanglement at all, since
//!   nothing here borrows from anything the function itself created.
//! - The caller parses each `Option<String>` into a
//!   `roxmltree::Document` (keeping it alive in its own scope for as
//!   long as it needs `Styles`/`Numbering`/`Footnotes`/`Theme`) and
//!   calls [`wire_parts`] with the resulting `Option<Node>`s -- the
//!   same "caller owns the parsed `Document`, passes `Node`s in"
//!   shape every test harness in this crate already uses for the main
//!   document.
//!
//! ```no_run
//! # use roxmltree::Document;
//! # use calibre_ebooks::docx::container::Docx;
//! # use calibre_ebooks::docx::names::DocxNamespace;
//! # use calibre_ebooks::docx::numbering::Numbering;
//! # use calibre_ebooks::docx::footnotes::Footnotes;
//! # use calibre_ebooks::docx::settings::Settings;
//! # use calibre_ebooks::docx::styles::Styles;
//! # use calibre_ebooks::docx::tables::Tables;
//! # use calibre_ebooks::docx::theme::Theme;
//! # use calibre_ebooks::docx::read_styles::{read_raw_parts, wire_parts};
//! # fn go<R: std::io::Read + std::io::Seek>(mut docx: Docx<R>) -> Result<(), calibre_ebooks::docx::error::DocxError> {
//! let document_name = docx.document_name()?;
//! let relationships_by_type = docx.document_relationships()?.by_type.clone();
//! let ns = docx.namespace.clone();
//! let parts = read_raw_parts(&mut docx, &document_name, &relationships_by_type, &ns);
//!
//! // Each of these lives in the caller's own scope for as long as it's needed.
//! let settings_doc = parts.settings.as_deref().and_then(|s| Document::parse(s).ok());
//! let footnotes_doc = parts.footnotes.as_deref().and_then(|s| Document::parse(s).ok());
//! let endnotes_doc = parts.endnotes.as_deref().and_then(|s| Document::parse(s).ok());
//! let theme_doc = parts.theme.as_deref().and_then(|s| Document::parse(s).ok());
//! let styles_doc = parts.styles.as_deref().and_then(|s| Document::parse(s).ok());
//! let numbering_doc = parts.numbering.as_deref().and_then(|s| Document::parse(s).ok());
//!
//! let mut settings = Settings::new();
//! let mut footnotes = Footnotes::new();
//! let mut theme = Theme::new();
//! let mut styles = Styles::new(Tables::default());
//! let mut numbering = Numbering::new();
//! wire_parts(
//!     &mut settings, &mut footnotes, &mut theme, &mut styles, &mut numbering,
//!     settings_doc.as_ref().map(Document::root_element),
//!     footnotes_doc.as_ref().map(Document::root_element),
//!     parts.footnotes_rels,
//!     endnotes_doc.as_ref().map(Document::root_element),
//!     parts.endnotes_rels,
//!     theme_doc.as_ref().map(Document::root_element),
//!     styles_doc.as_ref().map(Document::root_element),
//!     numbering_doc.as_ref().map(Document::root_element),
//!     &ns,
//! );
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::rc::Rc;

use roxmltree::Node;

use super::container::{Docx, Relationships};
use super::footnotes::Footnotes;
use super::names::DocxNamespace;
use super::numbering::Numbering;
use super::settings::Settings;
use super::styles::Styles;
use super::theme::Theme;

/// Resolves one part's zip-internal name: `relationships_by_type`'s
/// entry for `rtype` if the main document declares that relationship
/// type, else `default_name` alongside `document_name` if that
/// exists, with one more fallback for a double-`word/word/...` path
/// some documents produce.
///
/// One deliberate difference from Python's `get_name`: its fallback
/// path has a latent bug -- `name = cname` assigns the *list*
/// `document_name.split('/')` (with its last element replaced), never
/// joining it back into a string, so a later `name.startswith(...)`
/// would raise `AttributeError` if that branch and the
/// `word/word`-prefix branch were ever both hit for the same part.
/// Rust's static typing has no equivalent "wrong type smuggled
/// through" mistake to reproduce, and reproducing a crash isn't
/// useful behavior to preserve regardless (this session's established
/// precedent, e.g. `docx/images.rs`'s module docs) -- this does the
/// evidently-intended `'/'.join(cname)`.
pub fn resolve_part_name<R: Read + Seek>(
    docx: &Docx<R>,
    document_name: &str,
    relationships_by_type: &HashMap<String, String>,
    rtype: &str,
    default_name: &str,
) -> Option<String> {
    let mut name = relationships_by_type.get(rtype).cloned();
    if name.is_none() {
        let mut parts: Vec<&str> = document_name.split('/').collect();
        if let Some(last) = parts.last_mut() {
            *last = default_name;
        }
        let candidate = parts.join("/");
        if docx.exists(&candidate) {
            name = Some(candidate);
        }
    }
    if let Some(n) = &name {
        if n.starts_with("word/word") && !docx.exists(n) {
            name = n.splitn(2, '/').nth(1).map(str::to_string);
        }
    }
    name
}

/// Every part [`read_raw_parts`] found and could read, as raw XML
/// text -- `None` where the part doesn't exist, isn't declared, or
/// failed to read. Matches Python's own `try: ... except KeyError:
/// self.log.warn(...)`, minus the warning: no logger is threaded
/// through this module, same as every other function in this crate
/// that silently drops what Python would have logged.
#[derive(Debug, Clone, Default)]
pub struct RawParts {
    pub settings: Option<String>,
    pub footnotes: Option<String>,
    pub footnotes_rels: Relationships,
    pub endnotes: Option<String>,
    pub endnotes_rels: Relationships,
    pub theme: Option<String>,
    pub styles: Option<String>,
    pub numbering: Option<String>,
    /// `numbering.xml`'s own relationships (its embedded picture
    /// bullets' image ids) -- Python's `self.rid_map` (stored inside
    /// `Numbering.__call__`, read later by `Level.css`). Not consumed
    /// by [`wire_parts`] yet: `Numbering::call`/`Level::css` don't
    /// take a `rid_map` at all yet (issue #289's still-open
    /// picture-bullet-CSS follow-up) -- returned here regardless so
    /// that follow-up doesn't also need to re-derive this name
    /// resolution.
    pub numbering_rels: Relationships,
}

/// Port of the real-I/O half of `Convert.read_styles`: resolves and
/// reads every part's raw text (plus footnotes/endnotes/numbering's
/// own relationships). See the module docs for why parsing and wiring
/// into `Styles`/`Numbering`/`Footnotes`/`Theme` ([`wire_parts`]) is a
/// separate step.
pub fn read_raw_parts<R: Read + Seek>(
    docx: &mut Docx<R>,
    document_name: &str,
    relationships_by_type: &HashMap<String, String>,
    ns: &DocxNamespace,
) -> RawParts {
    let mut parts = RawParts::default();

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("SETTINGS").unwrap_or(""),
        "settings.xml",
    ) {
        parts.settings = docx.read_str(&name).ok();
    }

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("FOOTNOTES").unwrap_or(""),
        "footnotes.xml",
    ) {
        if let Ok(raw) = docx.read_str(&name) {
            parts.footnotes_rels = docx.get_relationships(&name);
            parts.footnotes = Some(raw);
        }
    }

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("ENDNOTES").unwrap_or(""),
        "endnotes.xml",
    ) {
        if let Ok(raw) = docx.read_str(&name) {
            parts.endnotes_rels = docx.get_relationships(&name);
            parts.endnotes = Some(raw);
        }
    }

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("THEMES").unwrap_or(""),
        "theme1.xml",
    ) {
        parts.theme = docx.read_str(&name).ok();
    }

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("STYLES").unwrap_or(""),
        "styles.xml",
    ) {
        parts.styles = docx.read_str(&name).ok();
    }

    if let Some(name) = resolve_part_name(
        docx,
        document_name,
        relationships_by_type,
        ns.name("NUMBERING").unwrap_or(""),
        "numbering.xml",
    ) {
        if let Ok(raw) = docx.read_str(&name) {
            parts.numbering_rels = docx.get_relationships(&name);
            parts.numbering = Some(raw);
        }
    }

    parts
}

/// Wires already-parsed part roots into `settings`/`footnotes`/
/// `theme`/`styles`/`numbering`, in Python's real order: settings,
/// footnotes+endnotes together, theme, styles (loaded even when
/// `styles_root` is `None`, matching Python's `self.styles(None,
/// fonts, self.theme)` fallback -- `Styles::call` already treats
/// `None` as "no explicit styles, resolve pure defaults" the same
/// way), numbering (only when `numbering_root` is `Some`, matching
/// Python's own `if nname is not None:` guard -- unlike styles,
/// nothing calls `Numbering::call` with an absent root), then
/// [`Styles::resolve_numbering`].
///
/// `resolve_numbering` takes `Numbering` *by value* in this port
/// (Python passes the same shared, implicitly-reference-counted
/// `numbering` object both here and to everything after `__call__`
/// keeps using it for) -- so this clones `numbering` into it rather
/// than moving the caller's own copy, which every other consumer
/// (`apply_numbering_markup`, and this function's own caller
/// afterward) still needs.
///
/// Port of the wiring half of `Convert.read_styles`.
#[allow(clippy::too_many_arguments)]
pub fn wire_parts<'a, 'i>(
    settings: &mut Settings,
    footnotes: &mut Footnotes<'a, 'i>,
    theme: &mut Theme,
    styles: &mut Styles<'a, 'i>,
    numbering: &mut Numbering,
    settings_root: Option<Node<'a, 'i>>,
    footnotes_root: Option<Node<'a, 'i>>,
    footnotes_rels: Relationships,
    endnotes_root: Option<Node<'a, 'i>>,
    endnotes_rels: Relationships,
    theme_root: Option<Node<'a, 'i>>,
    styles_root: Option<Node<'a, 'i>>,
    numbering_root: Option<Node<'a, 'i>>,
    ns: &DocxNamespace,
) {
    if let Some(root) = settings_root {
        settings.read(root, ns);
    }

    footnotes.load(
        footnotes_root,
        Rc::new(footnotes_rels),
        endnotes_root,
        Rc::new(endnotes_rels),
        ns,
    );

    if let Some(root) = theme_root {
        theme.read(root, ns);
    }

    styles.call(styles_root, ns);

    if let Some(root) = numbering_root {
        numbering.call(root, styles.numbering_style_links(), ns);
    }

    styles.resolve_numbering(numbering.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn package(parts: &[(&str, &str)]) -> Docx<Cursor<Vec<u8>>> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, content) in parts {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        Docx::new(Cursor::new(buf)).unwrap()
    }

    const CONTENT_TYPES: &str = r#"<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    const RELS: &str = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    mod resolve_part_name_tests {
        use super::*;

        #[test]
        fn a_declared_relationship_wins_over_the_default_filename() {
            let docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
                ("word/custom_styles.xml", "<w:styles/>"),
            ]);
            let by_type = HashMap::from([(
                "STYLES_TYPE".to_string(),
                "word/custom_styles.xml".to_string(),
            )]);
            let name = resolve_part_name(
                &docx,
                "word/document.xml",
                &by_type,
                "STYLES_TYPE",
                "styles.xml",
            );
            assert_eq!(name.as_deref(), Some("word/custom_styles.xml"));
        }

        #[test]
        fn falls_back_to_the_default_filename_alongside_document_xml() {
            let docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
                ("word/styles.xml", "<w:styles/>"),
            ]);
            let name = resolve_part_name(
                &docx,
                "word/document.xml",
                &HashMap::new(),
                "STYLES_TYPE",
                "styles.xml",
            );
            assert_eq!(name.as_deref(), Some("word/styles.xml"));
        }

        #[test]
        fn no_declaration_and_no_default_file_is_none() {
            let docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
            ]);
            let name = resolve_part_name(
                &docx,
                "word/document.xml",
                &HashMap::new(),
                "STYLES_TYPE",
                "styles.xml",
            );
            assert!(name.is_none());
        }

        #[test]
        fn a_doubled_word_word_prefix_is_stripped_when_the_full_path_is_missing() {
            let docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
                ("word/styles.xml", "<w:styles/>"),
            ]);
            let by_type = HashMap::from([(
                "STYLES_TYPE".to_string(),
                "word/word/styles.xml".to_string(),
            )]);
            let name = resolve_part_name(
                &docx,
                "word/document.xml",
                &by_type,
                "STYLES_TYPE",
                "styles.xml",
            );
            assert_eq!(name.as_deref(), Some("word/styles.xml"));
        }
    }

    mod read_raw_parts_tests {
        use super::*;

        #[test]
        fn reads_every_declared_or_default_named_part() {
            let mut docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
                ("word/settings.xml", "<w:settings/>"),
                ("word/styles.xml", "<w:styles/>"),
                ("word/numbering.xml", "<w:numbering/>"),
                ("word/theme/theme1.xml", "<a:theme/>"),
            ]);
            let ns = DocxNamespace::default();
            let by_type = HashMap::from([(
                ns.name("THEMES").unwrap().to_string(),
                "word/theme/theme1.xml".to_string(),
            )]);

            let parts = read_raw_parts(&mut docx, "word/document.xml", &by_type, &ns);

            assert_eq!(parts.settings.as_deref(), Some("<w:settings/>"));
            assert_eq!(parts.styles.as_deref(), Some("<w:styles/>"));
            assert_eq!(parts.numbering.as_deref(), Some("<w:numbering/>"));
            assert_eq!(parts.theme.as_deref(), Some("<a:theme/>"));
            assert!(parts.footnotes.is_none());
            assert!(parts.endnotes.is_none());
        }

        #[test]
        fn footnotes_and_their_own_relationships_are_read_together() {
            let mut docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
                ("word/footnotes.xml", "<w:footnotes/>"),
                (
                    "word/_rels/footnotes.xml.rels",
                    r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="t" Target="media/image1.png"/></Relationships>"#,
                ),
            ]);
            let ns = DocxNamespace::default();

            let parts = read_raw_parts(&mut docx, "word/document.xml", &HashMap::new(), &ns);

            assert_eq!(parts.footnotes.as_deref(), Some("<w:footnotes/>"));
            assert_eq!(
                parts.footnotes_rels.by_id.get("rId9").map(String::as_str),
                Some("word/media/image1.png")
            );
        }

        #[test]
        fn a_missing_optional_part_is_left_as_none_without_erroring() {
            let mut docx = package(&[
                ("[Content_Types].xml", CONTENT_TYPES),
                ("_rels/.rels", RELS),
                ("word/document.xml", "<w:document/>"),
            ]);
            let ns = DocxNamespace::default();

            let parts = read_raw_parts(&mut docx, "word/document.xml", &HashMap::new(), &ns);

            assert!(parts.settings.is_none());
            assert!(parts.styles.is_none());
            assert!(parts.numbering.is_none());
            assert!(parts.theme.is_none());
        }
    }

    mod wire_parts_tests {
        use super::*;
        use crate::docx::tables::Tables;
        use roxmltree::Document;

        const DOC_OPEN: &str =
            r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

        #[test]
        fn a_loaded_style_sheet_ends_up_queryable_on_styles() {
            let styles_xml: &'static str = Box::leak(
                format!(r#"<w:styles {DOC_OPEN}><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#)
                    .into_boxed_str(),
            );
            let styles_doc = Document::parse(styles_xml).unwrap();
            let ns = DocxNamespace::default();

            let mut settings = Settings::new();
            let mut footnotes = Footnotes::new();
            let mut theme = Theme::new();
            let mut styles = Styles::new(Tables::default());
            let mut numbering = Numbering::new();

            wire_parts(
                &mut settings,
                &mut footnotes,
                &mut theme,
                &mut styles,
                &mut numbering,
                None,
                None,
                Relationships::default(),
                None,
                Relationships::default(),
                None,
                Some(styles_doc.root_element()),
                None,
                &ns,
            );

            assert!(styles.id_map.contains_key("Heading1"));
        }

        #[test]
        fn no_parts_loaded_still_leaves_everything_in_a_valid_default_state() {
            let ns = DocxNamespace::default();
            let mut settings = Settings::new();
            let mut footnotes = Footnotes::new();
            let mut theme = Theme::new();
            let mut styles = Styles::new(Tables::default());
            let mut numbering = Numbering::new();

            wire_parts(
                &mut settings,
                &mut footnotes,
                &mut theme,
                &mut styles,
                &mut numbering,
                None,
                None,
                Relationships::default(),
                None,
                Relationships::default(),
                None,
                None,
                None,
                &ns,
            );

            assert!(!footnotes.has_notes());
        }
    }
}
