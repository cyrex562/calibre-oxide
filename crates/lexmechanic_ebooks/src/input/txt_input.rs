//! Port of `calibre.ebooks.conversion.plugins.txt_input.TXTInput`
//! (issue #537).
//!
//! Replaces a prior ad-hoc implementation (guessed "is this markdown"
//! via a crude heuristic, ran everything through `pulldown_cmark`
//! regardless of real formatting, and hand-rolled its own HTML
//! escaper) with the real pipeline, matching upstream's own real
//! control flow: detect encoding (`chardet::detect`/BOM), replace
//! entities, normalize line endings, detect and apply the real
//! paragraph structure, dispatch on formatting type to the real
//! per-format converter (`txt::processor::{convert_markdown_with_metadata,
//! convert_textile, convert_basic}`), then feed the resulting HTML
//! through the real [`HTMLInput`] to build the OEB -- the exact same
//! "HTML-ize then delegate" shape `djvu_input.rs` already established
//! for issue #129.
//!
//! # Disclosed narrowings
//!
//! - **`.txtz` archives are not supported** -- real upstream extracts
//!   a zip, concatenates every `.txt`/`.textile`/`.md` file inside,
//!   and reads a `metadata.opf` sidecar for a formatting-type hint and
//!   cover path. No real driving need for this bundled-text format
//!   exists elsewhere in this port yet.
//! - **`formatting_type = 'heuristic'`** (real upstream's own
//!   `detect_formatting_type`'s fallback result -- the common case for
//!   ordinary prose, not a rare edge case) needs no special handling
//!   here: real upstream's own `txt_input.py` ALSO just calls
//!   `convert_basic` for this case, merely flagging
//!   `options.enable_heuristics = True` so a *separate*, *later*
//!   pipeline stage (`calibre.ebooks.conversion.utils.HeuristicProcessor`,
//!   applied generically to any format) does chapter-heading/italics/
//!   smart-punctuation detection on the resulting HTML. That later
//!   stage isn't part of this port's conversion pipeline at all yet,
//!   so this port's own output for "heuristic" text is real, valid,
//!   un-enhanced basic HTML -- narrower than what real calibre
//!   eventually produces for the same input, but not silently wrong.
//! - **`paragraph_type = 'unformatted'`** real upstream additionally
//!   runs a punctuation-based hard-line-unwrap pass
//!   (`HeuristicProcessor::punctuation_unwrap`, driven by
//!   `DocAnalysis::line_length`) before treating each line as its own
//!   paragraph. Neither `HeuristicProcessor` nor `DocAnalysis` exists
//!   in this port, so this case falls back to the same single-line
//!   paragraph handling `paragraph_type = 'single'` uses, without the
//!   unwrap step.
//! - **`fix_resources`** (relocating local images referenced by
//!   Markdown/Textile source text so they resolve under `output_dir`)
//!   is not implemented -- a real, separable piece of work; local
//!   image references in the input text are left as-is.
//! - **User-configurable options** (`formatting_type`/`paragraph_type`
//!   overrides, `preserve_spaces`, `txt_in_remove_indents`,
//!   `markdown_extensions`) aren't wired -- this crate's plugins don't
//!   thread an `OptionRecommendation`-driven options object through
//!   yet (issue #126). Only the `auto`-detected path (real upstream's
//!   own default for both options) is implemented.
//! - **Metadata** beyond `title`/`authors` (real upstream's own
//!   `get_file_type_metadata` step) is a practical no-op for plain
//!   `.txt`/`.textile` in real calibre too (no registered
//!   `metadata/txt.py` reader plugin there), so this port only sets a
//!   filename-derived title for those formats. Markdown front-matter
//!   metadata (title/authors/etc, from `convert_markdown_with_metadata`)
//!   IS real and used when present.

use crate::chardet::{detect, detect_bom};
use crate::html_entities::xml_replace_entities;
use crate::input::html_input::HTMLInput;
use crate::oeb::book::OEBBook;
use crate::txt::processor::{
    block_to_single_line, convert_basic, convert_markdown_with_metadata, convert_textile, detect_formatting_type, detect_paragraph_type, normalize_line_endings, separate_hard_scene_breaks,
    separate_paragraphs_print_formatted, separate_paragraphs_single_line, DEFAULT_MD_EXTENSIONS,
};
use anyhow::{Context, Result};
use encoding_rs::Encoding;
use std::fs;
use std::path::Path;

pub struct TXTInput;

impl TXTInput {
    pub fn new() -> Self {
        TXTInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let raw = fs::read(input_path).context("Failed to read input file")?;

        // File extension forces the formatting type and turns off
        // paragraph reformatting, matching real upstream exactly.
        let ext = input_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let forced_formatting = match ext.as_str() {
            "md" | "markdown" => Some("markdown"),
            "textile" => Some("textile"),
            _ => None,
        };

        let (body, encoding) = decode(&raw);
        let mut txt = body;
        txt = xml_replace_entities(&txt);
        txt = normalize_line_endings(&txt);
        let _ = encoding; // kept for a future log/diagnostic hook

        if forced_formatting.is_none() {
            txt = apply_paragraph_structure(&txt, detect_paragraph_type(&txt));
        }

        let formatting_type = forced_formatting.unwrap_or_else(|| detect_formatting_type(&txt));
        let title_stem = input_path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Unknown".to_string());

        let (mi, html) = match formatting_type {
            "markdown" => {
                let (mi, html) = convert_markdown_with_metadata(&txt, &title_stem, DEFAULT_MD_EXTENSIONS);
                (Some(mi), html)
            }
            "textile" => (None, convert_textile(&txt, &title_stem)),
            // "heuristic" and any other detected value: same real
            // fallback upstream's own txt_input.py uses -- see this
            // module's own doc for why no special handling is needed.
            _ => (None, convert_basic(&txt, &title_stem, 0)),
        };

        fs::create_dir_all(output_dir).context("Failed to create output directory")?;
        let temp_dir = tempfile::tempdir().context("Failed to create a temporary directory")?;
        let temp_html_path = temp_dir.path().join("index.html");
        fs::write(&temp_html_path, &html).context("Failed to write intermediate HTML")?;

        let mut book = HTMLInput::new().convert(&temp_html_path, output_dir)?;

        book.metadata.clear("title");
        book.metadata.clear("dc:title");
        book.metadata.clear("creator");
        book.metadata.clear("dc:creator");
        if let Some(mi) = mi.filter(|mi| !mi.title.is_empty() && mi.title != "Unknown") {
            book.metadata.add("title", &mi.title);
            for author in &mi.authors {
                book.metadata.add("creator", author);
            }
        } else {
            book.metadata.add("title", &title_stem);
        }

        Ok(book)
    }
}

impl Default for TXTInput {
    fn default() -> Self {
        Self::new()
    }
}

/// Port of the encoding-detection half of `convert`: BOM first, else
/// `chardet.detect` with the same gb2312-family-to-gbk override real
/// upstream applies inline (Microsoft Word mislabels GBK text as
/// gb2312; gbk is a superset, so decoding as gbk is strictly safer).
fn decode(raw: &[u8]) -> (String, String) {
    if let Some((body, enc)) = detect_bom(raw) {
        let encoding = Encoding::for_label(enc.as_bytes()).unwrap_or(encoding_rs::UTF_8);
        let (cow, _, _) = encoding.decode(body);
        return (cow.into_owned(), enc.to_string());
    }
    let sample = &raw[..raw.len().min(4096)];
    let mut detected = detect(sample).encoding;
    if matches!(detected.to_lowercase().replace('_', "-").trim(), "gb2312" | "chinese" | "csiso58gb231280" | "euc-cn" | "euccn" | "eucgb2312-cn" | "gb2312-1980" | "gb2312-80" | "iso-ir-58") {
        detected = "gbk".to_string();
    }
    let encoding = Encoding::for_label(detected.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (cow, _, _) = encoding.decode(raw);
    (cow.into_owned(), detected)
}

/// Port of `convert`'s paragraph-type dispatch (the `single`/`print`/
/// `unformatted`/`block` branches). See this module's own doc for the
/// disclosed `unformatted` narrowing.
fn apply_paragraph_structure(txt: &str, paragraph_type: &str) -> String {
    match paragraph_type {
        "single" | "unformatted" => separate_paragraphs_single_line(txt),
        "print" => {
            let t = separate_hard_scene_breaks(txt);
            let t = separate_paragraphs_print_formatted(&t);
            block_to_single_line(&t)
        }
        _ => {
            let t = separate_hard_scene_breaks(txt);
            block_to_single_line(&t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(dir: &tempfile::TempDir, filename: &str, content: &[u8]) -> OEBBook {
        let input_path = dir.path().join(filename);
        fs::write(&input_path, content).unwrap();
        let output_dir = dir.path().join("out");
        TXTInput::new().convert(&input_path, &output_dir).expect("conversion failed")
    }

    #[test]
    fn plain_text_becomes_real_paragraphs_via_the_real_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let book = convert(&dir, "story.txt", b"First paragraph.\n\nSecond paragraph.");
        assert!(!book.manifest.items.is_empty());
        let mut found = false;
        for item in book.manifest.items.values() {
            if let Ok(content) = fs::read_to_string(dir.path().join("out").join(&item.href)) {
                if content.contains("First paragraph.") && content.contains("Second paragraph.") {
                    found = true;
                }
            }
        }
        assert!(found, "the real paragraph text should survive through to the HTML output");
        let titles = book.metadata.get("title");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value, "story");
    }

    #[test]
    fn markdown_front_matter_metadata_is_used_for_real() {
        let dir = tempfile::tempdir().unwrap();
        let md = b"title: My Real Title\nauthors: Jane Doe\n\n# Chapter One\n\nSome text.";
        let book = convert(&dir, "book.md", md);
        let titles = book.metadata.get("title");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].value, "My Real Title");
        let authors = book.metadata.get("creator");
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].value, "Jane Doe");
    }

    #[test]
    fn textile_markup_is_converted_via_the_real_textile_port() {
        let dir = tempfile::tempdir().unwrap();
        let book = convert(&dir, "story.textile", b"h1. A Heading\n\np. Some *bold* text.");
        let mut found_heading = false;
        for item in book.manifest.items.values() {
            if let Ok(content) = fs::read_to_string(dir.path().join("out").join(&item.href)) {
                if content.contains("<h1") && content.contains("A Heading") {
                    found_heading = true;
                }
            }
        }
        assert!(found_heading, "real Textile h1. syntax should become a real <h1>");
    }

    #[test]
    fn a_utf8_bom_is_stripped_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = vec![0xEF, 0xBB, 0xBF];
        content.extend_from_slice(b"Hello world.");
        let book = convert(&dir, "bom.txt", &content);
        let mut found = false;
        for item in book.manifest.items.values() {
            if let Ok(content) = fs::read_to_string(dir.path().join("out").join(&item.href)) {
                if content.contains("Hello world.") && !content.contains('\u{feff}') {
                    found = true;
                }
            }
        }
        assert!(found, "the BOM should not leak into the output text");
    }

    #[test]
    fn xml_entities_in_plain_text_are_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let book = convert(&dir, "entities.txt", "Caf&eacute; &amp; friends".as_bytes());
        let mut found = false;
        for item in book.manifest.items.values() {
            if let Ok(content) = fs::read_to_string(dir.path().join("out").join(&item.href)) {
                if content.contains("Café") {
                    found = true;
                }
            }
        }
        assert!(found, "named XML/HTML entities should be resolved before HTML-izing");
    }
}
