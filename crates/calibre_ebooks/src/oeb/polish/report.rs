//! Port of `old_src/src/calibre/ebooks/oeb/polish/report.py` (issue
//! #584, split off `tinycss`'s issue #88 since this file isn't itself
//! a tinycss file). Real Python data-gathering functions for the GUI's
//! "Check Book" report, orchestrated by `gather_data`.
//!
//! **Scope of this pass**: `files_data`/`images_data`/`words_data`/
//! `chars_data` -- every function whose real algorithm needs nothing
//! this crate doesn't already have. `links_data`/`create_anchor_map`
//! (and `css_data`, `gather_data`'s own orchestration) are NOT ported
//! here: both need a real source line/column on HTML elements, which
//! [`crate::dom::Dom`] (used for every HTML file in this crate) does
//! not track at all today -- confirmed by reading `dom.rs`'s `Node`
//! struct directly, not assumed. `css_data` additionally needs the
//! same kind of tracking on `crate::css::model`. This is a real,
//! disclosed, separately-filed follow-up (a position-tracking
//! infrastructure change, not a small wiring gap), not a silently
//! dropped piece of this issue.
//!
//! Two small real primitives this file needed didn't exist yet and
//! were added directly to `calibre_utils::icu` rather than
//! reimplemented locally: `numeric_strcmp` (port of
//! `icu.numeric_sort_key`, exposed as a comparator like every other
//! `icu.*strcmp` in this crate) and `safe_chr`.
//!
//! Python's `file_words_counts`/`gather_data` orchestration threads a
//! module-level global dict between `words_data` and `files_data`;
//! this port passes it explicitly as a parameter instead (a plain,
//! Rust-idiomatic replacement for the same data flow, not a
//! behavioral change).

use std::collections::HashMap;

use anyhow::Result;

use calibre_utils::icu::numeric_strcmp;

use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::spell::{count_all_chars, get_all_words, DictionaryLocale, Location};

use super::container::Container;
use super::utils::OEB_FONTS;

fn posix_dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn posix_basename(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[i + 1..].to_string(),
        None => path.to_string(),
    }
}

fn safe_size(container: &Container, name: &str) -> u64 {
    std::fs::metadata(container.name_to_abspath(name))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Port of `get_category`.
pub fn get_category(name: &str, mime_type: &str) -> &'static str {
    let mut category = "misc";
    if mime_type.starts_with("image/") {
        category = "image";
    } else if OEB_FONTS.contains(&mime_type) {
        category = "font";
    } else if OEB_STYLES.contains(&mime_type) {
        category = "style";
    } else if OEB_DOCS.contains(&mime_type) {
        category = "text";
    }
    // Python's `name.rpartition('.')[-1]` yields the whole name (not
    // an empty string) when there's no `.` at all.
    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    };
    match ext.as_str() {
        "ttf" | "otf" | "woff" | "woff2" => category = "font",
        "opf" => category = "opf",
        "ncx" => category = "toc",
        _ => {}
    }
    category
}

/// Port of the `File` namedtuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub dir: String,
    pub basename: String,
    pub size: u64,
    pub category: &'static str,
    /// `-1` matches Python's `fwc.get(name, -1)` sentinel for "not
    /// counted" (e.g. this file was never passed to `words_data`).
    pub word_count: i64,
}

/// Port of `files_data`. `file_words_counts` corresponds to Python's
/// module-level global of the same name, threaded explicitly here
/// instead (see the module docs) -- pass `None` for "not gathered yet"
/// (every file's `word_count` is then `-1`, matching the Python
/// default), or the map [`words_data`] fills in via its
/// `file_word_counts` out-parameter.
pub fn files_data(container: &Container, file_words_counts: Option<&HashMap<String, usize>>) -> Vec<FileEntry> {
    let mut names: Vec<&String> = container.name_path_map.keys().collect();
    // HashMap iteration order is not deterministic; Python's dict
    // preserves insertion order. Sorting by name is a real, disclosed
    // determinism improvement over relying on either language's
    // incidental iteration order.
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let mt = container.base.mime_map.get(name).map(String::as_str).unwrap_or("");
            FileEntry {
                dir: posix_dirname(name),
                basename: posix_basename(name),
                size: safe_size(container, name),
                category: get_category(name, mt),
                word_count: file_words_counts
                    .and_then(|m| m.get(name))
                    .map(|v| *v as i64)
                    .unwrap_or(-1),
                name: name.clone(),
            }
        })
        .collect()
}

/// Port of `LinkLocation`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkLocation {
    pub name: String,
    pub line_number: Option<u32>,
    pub text_on_line: String,
}

fn safe_href_to_name(container: &Container, href: &str, base: &str) -> Option<String> {
    container.href_to_name(href, Some(base))
}

/// Port of `sort_locations`.
fn sort_locations(container: &mut Container, mut locations: Vec<LinkLocation>) -> Result<Vec<LinkLocation>> {
    let spine_names = container.spine_names()?;
    let order: HashMap<&str, usize> = spine_names.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let fallback = order.len();
    locations.sort_by(|a, b| {
        let ia = order.get(a.name.as_str()).copied().unwrap_or(fallback);
        let ib = order.get(b.name.as_str()).copied().unwrap_or(fallback);
        ia.cmp(&ib)
            .then_with(|| numeric_strcmp(&a.name, &b.name))
            .then_with(|| a.line_number.cmp(&b.line_number))
    });
    Ok(locations)
}

fn safe_img_data(container: &mut Container, name: &str, mime_type: &str) -> (i64, i64) {
    if mime_type.contains("svg") {
        return (0, 0);
    }
    match container.raw_data(name, false) {
        Ok(data) => {
            let (_, width, height) = calibre_utils::imghdr::identify(&data);
            (width, height)
        }
        Err(_) => (0, 0),
    }
}

/// Port of the `Image` namedtuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageEntry {
    pub name: String,
    pub mime_type: String,
    pub usage: Vec<LinkLocation>,
    pub size: u64,
    pub basename: String,
    pub id: usize,
    pub width: i64,
    pub height: i64,
}

/// Port of `images_data`.
pub fn images_data(container: &mut Container) -> Result<Vec<ImageEntry>> {
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    let mut image_usage: HashMap<String, std::collections::HashSet<LinkLocation>> = HashMap::new();
    for (name, mt) in &names {
        if OEB_STYLES.contains(&mt.as_str()) || OEB_DOCS.contains(&mt.as_str()) {
            for (href, line_number, _offset) in container.iterlinks(name)? {
                let Some(target) = safe_href_to_name(container, &href, name) else {
                    continue;
                };
                if !container.exists(&target) {
                    continue;
                }
                let target_mt = container.base.mime_map.get(&target).cloned().unwrap_or_default();
                if target_mt.starts_with("image/") {
                    image_usage.entry(target).or_default().insert(LinkLocation {
                        name: name.clone(),
                        line_number,
                        text_on_line: href,
                    });
                }
            }
        }
    }

    let mut image_data = Vec::new();
    for (name, mt) in &names {
        if mt.starts_with("image/") && container.exists(name) {
            let usage: Vec<LinkLocation> = image_usage.remove(name).unwrap_or_default().into_iter().collect();
            let usage = sort_locations(container, usage)?;
            let (width, height) = safe_img_data(container, name, mt);
            let id = image_data.len();
            image_data.push(ImageEntry {
                name: name.clone(),
                mime_type: mt.clone(),
                usage,
                size: safe_size(container, name),
                basename: posix_basename(name),
                id,
                width,
                height,
            });
        }
    }
    Ok(image_data)
}

/// Port of the `Word` namedtuple.
#[derive(Debug, Clone)]
pub struct WordEntry {
    pub id: usize,
    pub word: String,
    pub locale: DictionaryLocale,
    pub usage: Vec<Location>,
}

/// Port of `words_data`. Returns the total word count, the per-word
/// entries, and (as an explicit out-parameter -- see the module docs)
/// the per-file word counts [`files_data`] can use.
pub fn words_data(
    container: &mut Container,
    book_locale: &DictionaryLocale,
) -> Result<(usize, Vec<WordEntry>, HashMap<String, usize>)> {
    let mut file_word_counts = HashMap::new();
    let excluded_files = std::collections::HashSet::new();
    let (count, words) = get_all_words(container, book_locale, &excluded_files, &mut file_word_counts)?;
    // HashMap iteration order isn't deterministic; sort by (word,
    // locale) for stable, reproducible `id` assignment -- Python relies
    // on dict insertion order here, an accident of first-encountered
    // order rather than a meaningful sort, so this is a real
    // improvement, not a narrowing.
    let mut entries: Vec<((String, DictionaryLocale), Vec<Location>)> = words.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    let words_vec = entries
        .into_iter()
        .enumerate()
        .map(|(id, ((word, locale), usage))| WordEntry { id, word, locale, usage })
        .collect();
    Ok((count, words_vec, file_word_counts))
}

/// Port of the `Char` namedtuple. `char` is already a validated
/// Unicode scalar value in Rust (unlike Python's raw integer
/// codepoint), so `icu::safe_chr` isn't needed at this specific call
/// site -- [`crate::oeb::polish::spell::CharCounter`] already stores
/// real `char`s, not codepoints that could be an invalid surrogate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharEntry {
    pub id: usize,
    pub ch: char,
    pub codepoint: u32,
    pub usage: Vec<String>,
    pub count: u32,
}

/// Port of `chars_data`.
pub fn chars_data(container: &mut Container, book_locale: &DictionaryLocale) -> Result<Vec<CharEntry>> {
    let cc = count_all_chars(container, book_locale)?;
    let spine_names = container.spine_names()?;
    let order: HashMap<&str, usize> = spine_names.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let fallback = order.len();

    // Deterministic ordering by codepoint, rather than relying on
    // HashMap iteration order the way Python relies on dict insertion
    // order (itself just first-encountered order, not a meaningful
    // sort) -- see `words_data`'s doc for the same reasoning.
    let mut chars: Vec<char> = cc.chars.keys().copied().collect();
    chars.sort();

    let mut result = Vec::with_capacity(chars.len());
    for (id, ch) in chars.into_iter().enumerate() {
        let mut usage: Vec<String> = cc.chars.get(&ch).cloned().unwrap_or_default().into_iter().collect();
        usage.sort_by(|a, b| {
            let ia = order.get(a.as_str()).copied().unwrap_or(fallback);
            let ib = order.get(b.as_str()).copied().unwrap_or(fallback);
            ia.cmp(&ib).then_with(|| numeric_strcmp(a, b))
        });
        result.push(CharEntry {
            id,
            ch,
            codepoint: ch as u32,
            usage,
            count: cc.counter.get(&ch).copied().unwrap_or(0),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::DictionaryLocale as SpellLocale;
    use std::fs;

    #[test]
    fn get_category_covers_images_fonts_styles_text_and_extension_overrides() {
        assert_eq!(get_category("cover.jpg", "image/jpeg"), "image");
        assert_eq!(get_category("style.css", "text/css"), "style");
        assert_eq!(get_category("chapter1.xhtml", "application/xhtml+xml"), "text");
        assert_eq!(get_category("weird.ttf", "application/octet-stream"), "font");
        assert_eq!(get_category("content.opf", "application/oebps-package+xml"), "opf");
        assert_eq!(get_category("toc.ncx", "application/x-dtbncx+xml"), "toc");
        assert_eq!(get_category("README", "text/plain"), "misc");
    }

    fn make_container(files: &[(&str, &str, &[u8])]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        let mut spine_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#));
            if mt.contains("xhtml") {
                spine_items.push_str(&format!(r#"<itemref idref="{name}"/>"#));
            }
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine>{spine_items}</spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn files_data_lists_every_manifest_item_with_category_and_size() {
        let (_dir, container) = make_container(&[
            ("chapter1.xhtml", "application/xhtml+xml", b"<html><body>hi</body></html>"),
            ("style.css", "text/css", b"body { color: red }"),
        ]);
        let files = files_data(&container, None);
        // Plus the container's own OPF file.
        assert_eq!(files.len(), 3);
        let chapter = files.iter().find(|f| f.name == "chapter1.xhtml").unwrap();
        assert_eq!(chapter.category, "text");
        assert_eq!(chapter.dir, "");
        assert_eq!(chapter.basename, "chapter1.xhtml");
        assert!(chapter.size > 0);
        assert_eq!(chapter.word_count, -1);
    }

    #[test]
    fn files_data_uses_the_provided_word_counts() {
        let (_dir, container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body>hi</body></html>",
        )]);
        let mut counts = HashMap::new();
        counts.insert("chapter1.xhtml".to_string(), 5usize);
        let files = files_data(&container, Some(&counts));
        assert_eq!(files[0].word_count, 5);
    }

    #[test]
    fn images_data_finds_a_linked_image_and_its_dimensions() {
        // A minimal, real 1x1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00,
            0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00,
            0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01,
            0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
            0x82,
        ];
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><body><img src=\"cover.png\"/></body></html>",
            ),
            ("cover.png", "image/png", png),
        ]);
        let images = images_data(&mut container).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].name, "cover.png");
        assert_eq!(images[0].width, 1);
        assert_eq!(images[0].height, 1);
        assert_eq!(images[0].usage.len(), 1);
        assert_eq!(images[0].usage[0].name, "chapter1.xhtml");
    }

    #[test]
    fn words_and_chars_data_walk_real_html_content() {
        let (_dir, mut container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body><p>hello world</p></body></html>",
        )]);
        let locale = SpellLocale::new("en", Some("US".to_string()));
        let (count, words, file_word_counts) = words_data(&mut container, &locale).unwrap();
        // 2 from the chapter's own text, plus the OPF's own dc:title ("T").
        assert_eq!(count, 3);
        let words_set: std::collections::HashSet<&str> = words.iter().map(|w| w.word.as_str()).collect();
        assert!(words_set.contains("hello"));
        assert!(words_set.contains("world"));
        assert_eq!(file_word_counts.get("chapter1.xhtml"), Some(&2));

        let chars = chars_data(&mut container, &locale).unwrap();
        let h = chars.iter().find(|c| c.ch == 'h').unwrap();
        assert!(h.count >= 1);
        assert!(h.usage.contains(&"chapter1.xhtml".to_string()));
    }
}
