//! CHM input.
//!
//! This used to be a placeholder that wrote a page reading "CHM Content
//! Not Supported Yet" and returned it as the book -- while registered, so
//! `ebook-convert book.chm out.epub` reported success and produced that
//! page instead of the book.
//!
//! Its stated reason was wrong: it said a full implementation "would need
//! a CHM crate (e.g. chm-rs if it existed/was mature)". `libchm` has been
//! a dependency of this crate the whole time, wrapped by
//! [`crate::chm::reader::ChmReader`] (`open`/`read_file`/`get_home`/
//! `list_files`), ported from `chm/reader.py`. So the engine was present
//! and unreached -- the same pattern as #926's PDB debug dump and #940's
//! DOCX output stub.

use crate::chm::reader::ChmReader;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::path::Path;

pub struct CHMInput;

impl CHMInput {
    pub fn new() -> Self {
        CHMInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        std::fs::create_dir_all(output_dir)?;

        let mut reader = ChmReader::open(input_path).with_context(|| format!("opening {}", input_path.display()))?;
        let title = reader.system().title_bytes.as_deref().map(decode_title);
        let default_topic = reader.system().default_topic.clone();

        // Everything is extracted, not just the HTML: a help file's pages
        // reference its images and stylesheets, and a book missing those
        // renders as unstyled text with broken images.
        let entries = reader.list_files().context("listing the CHM's entries")?;
        let mut extracted: Vec<String> = Vec::new();
        for entry in &entries {
            let relative = entry.trim_start_matches('/');
            if relative.is_empty() || relative.ends_with('/') {
                continue;
            }
            // CHM metadata entries (`#SYSTEM`, `$OBJINST`, …) are the
            // archive's own bookkeeping, not content.
            if relative.starts_with('#') || relative.starts_with('$') {
                continue;
            }
            let Ok(data) = reader.read_file(entry) else {
                // One unreadable entry does not abandon the book -- a help
                // file with a corrupt image is still worth converting.
                log::warn!("could not read {entry} from {}", input_path.display());
                continue;
            };
            // Entry paths come from a file the user did not write, so
            // they are untrusted input. `output_dir.join("../../x")`
            // escapes the output directory and writes wherever the process
            // can reach -- the zip-slip shape, and an ebook converter
            // handles files off the internet by definition.
            let Some(destination) = super::safe_join(output_dir, relative) else {
                log::warn!("skipping CHM entry that escapes the output directory: {entry}");
                continue;
            };
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&destination, &data)?;
            extracted.push(relative.to_string());
        }

        if extracted.is_empty() {
            anyhow::bail!("{} contains no readable entries", input_path.display());
        }

        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);
        book.metadata.add("title", title.as_deref().unwrap_or("Unknown"));

        // The default topic goes first in the spine: it is the CHM's own
        // declared entry point, and starting anywhere else opens the book
        // in the middle.
        let start = default_topic.as_deref().map(|t| t.trim_start_matches('/').to_string());
        let mut html: Vec<String> = extracted.iter().filter(|p| is_html(p)).cloned().collect();
        html.sort();
        if let Some(start) = start.as_deref() {
            if let Some(at) = html.iter().position(|p| p == start) {
                let first = html.remove(at);
                html.insert(0, first);
            }
        }

        for (index, href) in extracted.iter().enumerate() {
            let id = format!("item_{index}");
            let media_type = media_type_for(href);
            book.manifest.items.insert(id.clone(), ManifestItem::new(&id, href, media_type));
            book.manifest.hrefs.insert(href.clone(), id);
        }
        // Spine order follows the HTML list, so the default topic leads.
        for href in &html {
            if let Some(id) = book.manifest.hrefs.get(href) {
                let id = id.clone();
                book.spine.add(&id, true);
            }
        }

        Ok(book)
    }
}

fn is_html(path: &str) -> bool {
    matches!(extension_of(path).as_str(), "htm" | "html" | "xhtml")
}

fn extension_of(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// Media type for a manifest entry.
///
/// A CHM carries whatever the help author put in it, so this covers the
/// types that actually appear and falls back rather than refusing: an
/// unrecognised file is still worth keeping in the manifest, because the
/// HTML may reference it.
fn media_type_for(path: &str) -> &'static str {
    match extension_of(path).as_str() {
        "htm" | "html" | "xhtml" => "application/xhtml+xml",
        "css" => "text/css",
        "js" => "application/javascript",
        "gif" => "image/gif",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Decodes a CHM's title bytes.
///
/// `ChmSystemInfo` keeps them raw because the correct encoding depends on
/// the CHM's LCID, which this port does not read. UTF-8 is tried first and
/// Latin-1 used as the fallback -- between them they cover ASCII titles
/// exactly, which is the overwhelming majority, and produce readable
/// mojibake rather than an error for the rest. Better than discarding a
/// title because its codepage is unknown.
fn decode_title(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.trim().to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect::<String>().trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The behavioural change that matters: a file this cannot read is now
    /// an error.
    ///
    /// The placeholder produced a book for *anything* -- a page reading
    /// "CHM Content Not Supported Yet" -- and returned `Ok`, so a
    /// conversion of a corrupt or non-CHM file reported success and handed
    /// the user that page as their book.
    #[test]
    fn a_file_that_is_not_a_chm_is_refused_rather_than_producing_a_page() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("book.chm");
        std::fs::write(&fake, b"not a CHM file").unwrap();
        let out = dir.path().join("out");

        let result = CHMInput::new().convert(&fake, &out);
        assert!(result.is_err(), "a non-CHM must not convert to a placeholder page");

        // And specifically not the old placeholder.
        let index = out.join("index.html");
        if index.exists() {
            let text = std::fs::read_to_string(&index).unwrap();
            assert!(!text.contains("Not Supported Yet"), "the placeholder page is still being written:\n{text}");
        }
    }

    /// A CHM is a file the user did not write, so its entry paths are
    /// untrusted. Joining one straight onto the output directory is the
    /// zip-slip shape: `output_dir.join("../../x")` escapes and writes
    /// wherever the process can reach.
    #[test]
    fn an_entry_that_escapes_the_output_directory_is_rejected() {
        let base = Path::new("/tmp/out");
        for hostile in [
            "../escaped.html",
            "../../escaped.html",
            "a/../../escaped.html",
            "a/b/../../../escaped.html",
            "/absolute.html",
            "..",
            ".",
            "",
        ] {
            assert_eq!(super::super::safe_join(base, hostile), None, "{hostile:?} should have been rejected");
        }
    }

    /// The exact shape the old code let through: leading slashes were
    /// stripped with `trim_start_matches('/')`, which turns
    /// `/../../etc/x` into `../../etc/x` and then joins it.
    #[test]
    fn stripping_a_leading_slash_does_not_reopen_the_escape() {
        let base = Path::new("/tmp/out");
        let entry = "/../../etc/passwd";
        let stripped = entry.trim_start_matches('/');
        assert_eq!(stripped, "../../etc/passwd", "this is what the old code produced");
        assert_eq!(super::super::safe_join(base, stripped), None, "and it must not be joinable");
    }

    /// Ordinary entries still work, including nested ones and `./`.
    #[test]
    fn ordinary_entries_are_joined_under_the_output_directory() {
        let base = Path::new("/tmp/out");
        assert_eq!(super::super::safe_join(base, "index.html"), Some(base.join("index.html")));
        assert_eq!(super::super::safe_join(base, "images/logo.png"), Some(base.join("images/logo.png")));
        assert_eq!(super::super::safe_join(base, "./index.html"), Some(base.join("index.html")));
        assert_eq!(super::super::safe_join(base, "a/./b/c.html"), Some(base.join("a/b/c.html")));
    }

    /// Every result stays under the base -- the property the guard exists
    /// for, asserted directly rather than inferred from the rejections.
    #[test]
    fn every_accepted_path_stays_under_the_base() {
        let base = Path::new("/tmp/out");
        for entry in ["index.html", "a/b.png", "./x/y/z.css", "weird..name.html", "..hidden.html"] {
            if let Some(joined) = super::super::safe_join(base, entry) {
                assert!(joined.starts_with(base), "{entry:?} produced {joined:?}, which is outside {base:?}");
            }
        }
    }

    #[test]
    fn a_missing_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(CHMInput::new().convert(&dir.path().join("absent.chm"), &dir.path().join("out")).is_err());
    }

    /// Only HTML goes in the spine; everything else stays in the manifest
    /// so the HTML's references resolve.
    #[test]
    fn html_is_recognised_for_the_spine_and_other_files_are_not() {
        for html in ["index.htm", "page.html", "a/b/c.xhtml", "UPPER.HTML"] {
            assert!(is_html(html), "{html} should be spine content");
        }
        for other in ["style.css", "logo.png", "script.js", "data.bin", "noextension"] {
            assert!(!is_html(other), "{other} should not be spine content");
        }
    }

    /// An unrecognised type still gets a manifest entry -- the HTML may
    /// reference it, and dropping it would break the page.
    #[test]
    fn media_types_cover_what_a_help_file_contains_and_fall_back_otherwise() {
        assert_eq!(media_type_for("index.html"), "application/xhtml+xml");
        assert_eq!(media_type_for("style.css"), "text/css");
        assert_eq!(media_type_for("logo.PNG"), "image/png");
        assert_eq!(media_type_for("photo.jpeg"), "image/jpeg");
        assert_eq!(media_type_for("old.bmp"), "image/bmp");
        assert_eq!(media_type_for("mystery.xyz"), "application/octet-stream");
        assert_eq!(media_type_for("noextension"), "application/octet-stream");
    }

    /// `ChmSystemInfo` keeps the title as raw bytes because the encoding
    /// depends on an LCID this port does not read. ASCII must come through
    /// exactly; anything else should degrade rather than be discarded.
    #[test]
    fn the_title_decodes_ascii_exactly_and_falls_back_for_the_rest() {
        assert_eq!(decode_title(b"A Help File"), "A Help File");
        assert_eq!(decode_title(b"  padded  "), "padded");
        // Valid UTF-8 is taken as UTF-8.
        assert_eq!(decode_title("Café".as_bytes()), "Café");
        // Invalid UTF-8 falls back to Latin-1 rather than erroring.
        assert_eq!(decode_title(&[0x43, 0x61, 0x66, 0xe9]), "Café");
    }
}
