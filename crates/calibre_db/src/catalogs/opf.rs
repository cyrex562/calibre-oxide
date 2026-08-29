//! Port of `epub_mobi_builder.py`'s `generate_opf` -- cluster E of the
//! `epub_mobi_builder.py` port (issue #57; see `epub_mobi_builder.rs`'s
//! own module doc for clusters A-D, already complete). Split into its
//! own file for the same reason as `ncx.rs` (`epub_mobi_builder.rs` is
//! already ~3900 lines) -- still conceptually part of the same
//! `CatalogBuilder` port.
//!
//! # Disclosed simplifications
//!
//! - **`lang` arrives already-resolved**, not derived from
//!   `get_lang()`/`lang_as_iso639_1()` (calibre's locale-detection
//!   subsystem, not ported anywhere in this crate). Matches this whole
//!   port's "pure function, caller supplies impure/environment-derived
//!   inputs" convention (same shape as `bibtex.rs`'s `generated_at`
//!   parameter for wall-clock reads).
//! - **No pretty-printing.** Same as `ncx.rs`'s `write_ncx` -- upstream's
//!   `pretty_opf`/`pretty_xml_tree` calls have no
//!   [`calibre_ebooks::dom::Dom`] equivalent; cosmetic only.

use serde_json::Value;

use calibre_ebooks::dom::{Dom, NodeId};

use super::epub_mobi_builder::GenrePage;

fn set_attr(dom: &mut Dom, id: NodeId, name: &str, value: impl Into<String>) {
    dom.node_mut(id).attrs.insert(name.to_string(), value.into());
}

fn append_text(dom: &mut Dom, parent: NodeId, text: &str) {
    let t = dom.new_text(text);
    dom.append_child(parent, t);
}

/// Port of the `start = file.find('/') + 1; end = file.find('.')`
/// manifest-id derivation shared by the HTML-file, genre-file, and
/// (separately, with different slicing) thumbnail-file manifest loops.
fn manifest_id_from_path(path: &str) -> String {
    let start = path.find('/').map(|i| i + 1).unwrap_or(0);
    let end = path.find('.').unwrap_or(path.len());
    path[start..end].to_lowercase()
}

/// Options this reads from `self.opts`/`self` -- see this module's doc
/// for what's simplified.
#[derive(Debug, Clone)]
pub struct OpfOptions {
    pub catalog_title: String,
    pub creator: String,
    pub lang: String,
    pub generate_for_kindle_mobi: bool,
    pub basename: String,
    pub stylesheet: String,
    pub generate_descriptions: bool,
}

/// Port of `generate_opf`. `thumbs` is `self.thumbs` (cluster E's
/// thumbnail-generation output, filenames like `"thumbnail_1.jpg"`);
/// `html_filelist_1`/`html_filelist_2` are the by-author/by-title/
/// by-series and by-date-added/by-date-read HTML file lists cluster C's
/// generators append to as they run; `genres` is
/// `generate_html_by_genres`'s own output; `books_by_description` is
/// `fetch_books_by_author`'s `books_by_description` field. Returns the
/// finished `.opf` document string -- writing it to `catalog_path` is
/// the caller's job (cluster F), matching this whole port's "pure
/// function, caller does I/O" convention.
pub fn generate_opf(
    opts: &OpfOptions,
    thumbs: &[String],
    html_filelist_1: &[String],
    genres: &[GenrePage],
    html_filelist_2: &[String],
    books_by_description: &[Value],
) -> String {
    let mut dom = Dom::empty();
    let root = dom.root;
    let package = dom.new_element("package");
    set_attr(&mut dom, package, "xmlns", "http://www.idpf.org/2007/opf");
    set_attr(&mut dom, package, "version", "2.0");
    set_attr(&mut dom, package, "unique-identifier", "calibre_id");
    dom.append_child(root, package);

    let metadata = dom.new_element("metadata");
    set_attr(&mut dom, metadata, "xmlns:dc", "http://purl.org/dc/elements/1.1/");
    set_attr(&mut dom, metadata, "xmlns:calibre", "http://calibre.kovidgoyal.net/2009/metadata");
    set_attr(&mut dom, metadata, "xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance");
    dom.append_child(package, metadata);

    let title_tag = dom.new_element("dc:title");
    append_text(&mut dom, title_tag, &opts.catalog_title);
    dom.append_child(metadata, title_tag);

    let creator_tag = dom.new_element("dc:creator");
    append_text(&mut dom, creator_tag, &opts.creator);
    dom.append_child(metadata, creator_tag);

    let lang_tag = dom.new_element("dc:language");
    append_text(&mut dom, lang_tag, &opts.lang);
    dom.append_child(metadata, lang_tag);

    let meta_pt = dom.new_element("meta");
    set_attr(&mut dom, meta_pt, "name", "calibre:publication_type");
    set_attr(&mut dom, meta_pt, "content", if opts.generate_for_kindle_mobi { "periodical:default" } else { "" });
    dom.append_child(metadata, meta_pt);

    let manifest = dom.new_element("manifest");
    dom.append_child(package, manifest);
    let spine = dom.new_element("spine");
    set_attr(&mut dom, spine, "toc", "ncx");
    dom.append_child(package, spine);
    let guide = dom.new_element("guide");
    dom.append_child(package, guide);

    let manifest_item = |dom: &mut Dom, id: &str, href: &str, media_type: &str, add_to_spine: bool| {
        let item = dom.new_element("item");
        set_attr(dom, item, "id", id);
        set_attr(dom, item, "href", href);
        set_attr(dom, item, "media-type", media_type);
        dom.append_child(manifest, item);
        if add_to_spine {
            let itemref = dom.new_element("itemref");
            set_attr(dom, itemref, "idref", id);
            dom.append_child(spine, itemref);
        }
    };

    manifest_item(&mut dom, "ncx", &format!("{}.ncx", opts.basename), "application/x-dtbncx+xml", false);
    manifest_item(&mut dom, "stylesheet", &opts.stylesheet, "text/css", false);

    if opts.generate_for_kindle_mobi {
        manifest_item(&mut dom, "mastheadimage-image", "images/mastheadImage.gif", "image/gif", false);
    }

    if opts.generate_descriptions {
        for thumb in thumbs {
            let Some(end) = thumb.find(".jpg") else { continue };
            let id = format!("{}-image", &thumb[..end]);
            manifest_item(&mut dom, &id, &format!("images/{thumb}"), "image/jpeg", false);
        }
    }

    for file in html_filelist_1 {
        manifest_item(&mut dom, &manifest_id_from_path(file), file, "application/xhtml+xml", true);
    }
    for genre in genres {
        manifest_item(&mut dom, &manifest_id_from_path(&genre.file), &genre.file, "application/xhtml+xml", true);
    }
    for file in html_filelist_2 {
        manifest_item(&mut dom, &manifest_id_from_path(file), file, "application/xhtml+xml", true);
    }
    for book in books_by_description {
        let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
        let href = format!("content/book_{book_id}.html");
        manifest_item(&mut dom, &format!("book{book_id}"), &href, "application/xhtml+xml", true);
    }

    if opts.generate_for_kindle_mobi {
        let reference = dom.new_element("reference");
        set_attr(&mut dom, reference, "type", "masthead");
        set_attr(&mut dom, reference, "title", "masthead-image");
        set_attr(&mut dom, reference, "href", "images/mastheadImage.gif");
        dom.append_child(guide, reference);
    }

    dom.serialize(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_opts() -> OpfOptions {
        OpfOptions {
            catalog_title: "My Books".to_string(),
            creator: "calibre".to_string(),
            lang: "en".to_string(),
            generate_for_kindle_mobi: false,
            basename: "catalog".to_string(),
            stylesheet: "content/stylesheet.css".to_string(),
            generate_descriptions: false,
        }
    }

    #[test]
    fn generate_opf_produces_a_well_formed_package() {
        let opf = generate_opf(&default_opts(), &[], &[], &[], &[], &[]);
        assert!(opf.contains("<package"), "{opf}");
        assert!(opf.contains("<dc:title>My Books</dc:title>"), "{opf}");
        assert!(opf.contains("<dc:creator>calibre</dc:creator>"), "{opf}");
        assert!(opf.contains("<dc:language>en</dc:language>"), "{opf}");
        assert!(opf.contains("id=\"ncx\""), "{opf}");
        assert!(opf.contains("href=\"catalog.ncx\""), "{opf}");
        assert!(opf.contains("href=\"content/stylesheet.css\""), "{opf}");
    }

    #[test]
    fn generate_opf_kindle_mobi_adds_masthead_and_publication_type() {
        let opts = OpfOptions { generate_for_kindle_mobi: true, ..default_opts() };
        let opf = generate_opf(&opts, &[], &[], &[], &[], &[]);
        assert!(opf.contains("mastheadimage-image"), "{opf}");
        assert!(opf.contains("content=\"periodical:default\""), "{opf}");
        assert!(opf.contains("type=\"masthead\""), "{opf}");
    }

    #[test]
    fn generate_opf_epub_has_no_masthead() {
        let opf = generate_opf(&default_opts(), &[], &[], &[], &[], &[]);
        assert!(!opf.contains("masthead"), "{opf}");
        assert!(opf.contains("content=\"\""), "{opf}");
    }

    #[test]
    fn generate_opf_html_files_get_lowercased_ids_and_join_the_spine() {
        let opf = generate_opf(&default_opts(), &[], &["content/ByAlphaAuthor.html".to_string()], &[], &[], &[]);
        assert!(opf.contains("id=\"byalphaauthor\""), "{opf}");
        assert!(opf.contains("href=\"content/ByAlphaAuthor.html\""), "{opf}");
        assert!(opf.contains("idref=\"byalphaauthor\""), "{opf}");
    }

    #[test]
    fn generate_opf_genre_files_are_added_to_the_manifest_and_spine() {
        let genres = vec![GenrePage {
            tag: "scifi".to_string(),
            file: "content/Genre_scifi.html".to_string(),
            authors: vec![],
            books: vec![],
            titles_spanned: vec![],
            html: String::new(),
        }];
        let opf = generate_opf(&default_opts(), &[], &[], &genres, &[], &[]);
        assert!(opf.contains("id=\"genre_scifi\""), "{opf}");
        assert!(opf.contains("idref=\"genre_scifi\""), "{opf}");
    }

    #[test]
    fn generate_opf_thumbnails_only_included_when_generating_descriptions() {
        let thumbs = vec!["thumbnail_default.jpg".to_string()];
        let without = generate_opf(&default_opts(), &thumbs, &[], &[], &[], &[]);
        assert!(!without.contains("thumbnail_default"), "{without}");

        let opts = OpfOptions { generate_descriptions: true, ..default_opts() };
        let with_thumbs = generate_opf(&opts, &thumbs, &[], &[], &[], &[]);
        assert!(with_thumbs.contains("id=\"thumbnail_default-image\""), "{with_thumbs}");
        assert!(with_thumbs.contains("href=\"images/thumbnail_default.jpg\""), "{with_thumbs}");
    }

    #[test]
    fn generate_opf_description_books_are_added_to_the_manifest_and_spine() {
        let books = vec![serde_json::json!({"id": 42})];
        let opf = generate_opf(&default_opts(), &[], &[], &[], &[], &books);
        assert!(opf.contains("id=\"book42\""), "{opf}");
        assert!(opf.contains("href=\"content/book_42.html\""), "{opf}");
        assert!(opf.contains("idref=\"book42\""), "{opf}");
    }
}
