//! Port of `calibre.ebooks.oeb.polish.main`'s real value: `polish_one`,
//! the "Polish Book" orchestrator that runs a book through whichever
//! subset of the already-ported polish actions (`cover.rs`/`css.rs`/
//! `download.rs`/`hyphenation.rs`/`images.rs`/`jacket.rs`/`replace.rs`/
//! `stats.rs`/`subset.rs`/`upgrade.rs`/`embed.rs`) an [`PolishOptions`]
//! value requests (issue #169, final follow-up to #39; every dependency
//! it lists -- #162-166, #168 -- is closed as of this port).
//!
//! # Disclosed narrowings
//!
//! - **`opts.opf` (replace metadata from an external OPF file)**: real
//!   upstream's `update_metadata` needs `calibre.ebooks.metadata.opf.set_metadata`
//!   -- an in-place metadata rewrite of an *existing* OPF tree
//!   (preserving manifest/spine/guide, only touching `<metadata>`).
//!   This port has an OPF *reader* (`crate::opf::parse_opf`) and a
//!   from-scratch OPF *builder* (`crate::opf_writer::write_opf`, which
//!   emits a brand new manifest/spine/guide), but not that surgical
//!   in-place rewrite. Returns a real error rather than silently
//!   skipping or reaching for the wrong tool.
//! - **`opts.embed`/`opts.subset`**: `embed_all_fonts`/`subset_all_fonts`
//!   are themselves real, documented `todo!()` gaps (a font-subsetting
//!   library this crate doesn't have) -- `polish_one` calls straight
//!   through to them, matching this crate's standing "real signature,
//!   `todo!()` body where genuinely blocked" convention, rather than
//!   intercepting the request with a different error here. `StatsCollector`
//!   itself (font *usage collection*, as opposed to the embed/subset
//!   *actions*) is real and is still run when either option is set,
//!   matching real upstream.
//! - **CLI/GUI layers not ported**: `option_parser`/`main`/`CLI_HELP`
//!   (CLI argument parsing -- needs #126's `OptionRecommendation`
//!   threading, which doesn't exist yet) and `gui_polish`/`tweak_polish`
//!   (thin GUI-integration wrappers around `polish_one` with no GUI to
//!   integrate with in this port) are out of scope. `polish()` (the
//!   multi-file batch driver that opens/commits each book) is also out
//!   of scope -- it's a few lines of boilerplate any real future caller
//!   can write directly against `get_container`/`AnyContainer::commit`-
//!   per-variant/[`polish_one`], not a piece with real logic of its own
//!   worth porting ahead of a caller that needs it.
//! - **`upgrade_book`'s own signature** requires `&mut EpubContainer`
//!   specifically (EPUB2->3 upgrade is EPUB-structure-specific), not
//!   the generic `&mut Container` every other action here takes. Real
//!   upstream's own `upgrade_book` no-ops for anything that isn't
//!   `book_type in ('epub', 'kepub')` -- ported the same effective
//!   scope by extracting `&mut EpubContainer` from either
//!   [`AnyContainer::Epub`] or [`AnyContainer::Kepub`] (which wraps one),
//!   and reporting the same "No upgrade needed" message [`AnyContainer::Azw3`]
//!   would have gotten from that same real book_type check.

use anyhow::{bail, Result};
use std::time::Duration;

use crate::oeb::polish::container::AnyContainer;
use crate::oeb::polish::cover::set_cover;
use crate::oeb::polish::css::{remove_unused_css, RemoveUnusedCssOptions};
use crate::oeb::polish::download::{download_external_resources, get_external_resources, replace_resources};
use crate::oeb::polish::embed::embed_all_fonts;
use crate::oeb::polish::hyphenation::{add_soft_hyphens, remove_soft_hyphens};
use crate::oeb::polish::images::compress_images;
use crate::oeb::polish::jacket::{add_or_replace_jacket, remove_jacket};
use crate::oeb::polish::replace::smarten_punctuation;
use crate::oeb::polish::stats::StatsCollector;
use crate::oeb::polish::subset::{iter_subsettable_fonts, subset_all_fonts};
use crate::oeb::polish::upgrade::upgrade_book;

/// Port of `ALL_OPTS`. Every field defaults `false`/`None`, matching
/// Python's own all-`False`/`None` dict.
#[derive(Debug, Clone, Default)]
pub struct PolishOptions {
    /// Path to a replacement cover image.
    pub cover: Option<String>,
    /// Path to an OPF file to read replacement metadata from -- see
    /// the module doc's disclosed narrowing; always errors if set.
    pub opf: Option<std::path::PathBuf>,
    pub jacket: bool,
    pub remove_jacket: bool,
    pub smarten_punctuation: bool,
    pub remove_unused_css: bool,
    pub compress_images: bool,
    pub upgrade_book: bool,
    pub add_soft_hyphens: bool,
    pub remove_soft_hyphens: bool,
    pub download_external_resources: bool,
    /// See the module doc's disclosed narrowing -- calling through to a
    /// real, documented `todo!()` gap when set.
    pub embed: bool,
    /// See the module doc's disclosed narrowing -- calling through to a
    /// real, documented `todo!()` gap when set.
    pub subset: bool,
}

/// Port of `CUSTOMIZATION`.
#[derive(Debug, Clone)]
pub struct PolishCustomization {
    pub remove_unused_classes: bool,
    pub merge_identical_selectors: bool,
    pub merge_rules_with_identical_properties: bool,
    pub remove_unreferenced_sheets: bool,
    pub remove_ncx: bool,
}

impl Default for PolishCustomization {
    fn default() -> Self {
        PolishCustomization {
            remove_unused_classes: false,
            merge_identical_selectors: false,
            merge_rules_with_identical_properties: false,
            remove_unreferenced_sheets: true,
            remove_ncx: true,
        }
    }
}

/// Book locale used for soft-hyphenation/spell-aware actions. Real
/// upstream threads `dictionaries.default_locale` (a GUI preference)
/// through; narrowed to a plain parameter here since no such GUI
/// preference store exists in this port.
const DEFAULT_LOCALE: &str = "en";

/// Port of `download_resources`: fetches every externally-referenced
/// resource in the book and rewrites references to the downloaded
/// copies. Returns whether anything changed.
fn download_resources(container: &mut AnyContainer, report: &mut dyn FnMut(&str)) -> Result<bool> {
    let c = container.as_container_mut();
    let url_to_referrers = get_external_resources(c)?;
    if url_to_referrers.is_empty() {
        report("No external resources found in book");
        return Ok(false);
    }
    let n = url_to_referrers.len();
    report(&format!(
        "Downloading {n} external resource{}",
        if n == 1 { "" } else { "s" }
    ));
    let urls: Vec<String> = url_to_referrers.keys().cloned().collect();
    let (replacements, failures) = download_external_resources(c, &urls, Duration::from_secs(30), |_, _, _| {})?;
    if failures.is_empty() {
        report("Successfully downloaded all resources");
    } else if replacements.is_empty() {
        report("Failed to download all resources, see details below:");
        for (url, err) in &failures {
            report(&format!("{url}\n\t{err}\n"));
        }
    } else {
        report("Failed to download some resources, see details below:");
        for (url, err) in &failures {
            report(&format!("{url}\n\t{err}\n"));
        }
    }
    if !replacements.is_empty() {
        return replace_resources(c, &url_to_referrers, &replacements);
    }
    Ok(false)
}

/// Port of `polish_one`: runs whichever actions `opts` requests against
/// a single already-opened book, in the same order real upstream does.
/// Returns whether anything actually changed.
pub fn polish_one(
    container: &mut AnyContainer,
    opts: &PolishOptions,
    report: &mut dyn FnMut(&str),
    customization: Option<&PolishCustomization>,
) -> Result<bool> {
    let default_customization = PolishCustomization::default();
    let customization = customization.unwrap_or(&default_customization);
    let rt = |report: &mut dyn FnMut(&str), x: &str| report(&format!("\n### {x}"));

    let mut changed = false;

    let has_subsettable_fonts = !iter_subsettable_fonts(container.as_container_mut())?.is_empty();

    let mut stats = if (opts.subset && has_subsettable_fonts) || opts.embed {
        Some(StatsCollector::new(container.as_container_mut(), opts.embed)?)
    } else {
        None
    };

    if opts.opf.is_some() {
        bail!(
            "Replacing metadata from an external OPF file is not yet supported: \
             this port has no in-place OPF <metadata> rewriter (calibre.ebooks.metadata.opf.set_metadata's \
             real equivalent), only a full-document reader and a from-scratch builder"
        );
    }

    if let Some(cover_path) = &opts.cover {
        changed = true;
        rt(report, "Setting cover");
        set_cover(container.as_container_mut(), cover_path, Some(report), None)?;
        report("");
    }

    if opts.jacket {
        changed = true;
        rt(report, "Inserting metadata jacket");
        // Real upstream also short-circuits this "did opts.opf already
        // find/replace a jacket?" check when `jacket` is `None` -- since
        // `opts.opf` always errors here (see this module's disclosed
        // narrowing), that's the only reachable path.
        if add_or_replace_jacket(container.as_container_mut())? {
            report("Existing metadata jacket replaced");
        } else {
            report("Metadata jacket inserted");
        }
        report("");
    }

    if opts.remove_jacket {
        rt(report, "Removing metadata jacket");
        if remove_jacket(container.as_container_mut())? {
            report("Metadata jacket removed");
            changed = true;
        } else {
            report("No metadata jacket found");
        }
        report("");
    }

    if opts.smarten_punctuation {
        rt(report, "Smartening punctuation");
        if smarten_punctuation(container.as_container_mut(), &mut *report)? {
            changed = true;
        }
        report("");
    }

    if opts.embed {
        rt(report, "Embedding referenced fonts");
        let c = container.as_container_mut();
        if embed_all_fonts(c, &[], report)? {
            changed = true;
        }
        report("");
    }

    if opts.subset {
        if has_subsettable_fonts {
            rt(report, "Subsetting embedded fonts");
            let font_stats = stats.take().map(|s| s.font_stats).unwrap_or_default();
            if subset_all_fonts(container.as_container_mut(), &font_stats, report)? {
                changed = true;
            }
        } else {
            rt(report, "No embedded fonts to subset");
        }
        report("");
    }

    if opts.remove_unused_css {
        rt(report, "Removing unused CSS rules");
        let css_opts = RemoveUnusedCssOptions {
            remove_unused_classes: customization.remove_unused_classes,
            merge_rules: customization.merge_identical_selectors,
            merge_rules_with_identical_properties: customization.merge_rules_with_identical_properties,
            remove_unreferenced_sheets: customization.remove_unreferenced_sheets,
        };
        if remove_unused_css(container.as_container_mut(), css_opts, &mut *report)? {
            changed = true;
        }
        report("");
    }

    if opts.compress_images {
        rt(report, "Losslessly compressing images");
        let (did_change, _outcomes) = compress_images(container.as_container_mut(), None, None, None, Some(report), |_, _, _| true)?;
        if did_change {
            changed = true;
        }
        report("");
    }

    if opts.upgrade_book {
        rt(report, "Upgrading book, if possible");
        match container {
            AnyContainer::Epub(c) => {
                if upgrade_book(c, &mut *report, customization.remove_ncx)? {
                    changed = true;
                }
            }
            AnyContainer::Kepub(c) => {
                if upgrade_book(&mut c.epub, &mut *report, customization.remove_ncx)? {
                    changed = true;
                }
            }
            AnyContainer::Azw3(_) => {
                report("No upgrade needed");
            }
        }
        report("");
    }

    if opts.remove_soft_hyphens {
        rt(report, "Removing soft hyphens");
        remove_soft_hyphens(container.as_container_mut(), Some(report))?;
        changed = true;
    } else if opts.add_soft_hyphens {
        rt(report, "Adding soft hyphens");
        add_soft_hyphens(container.as_container_mut(), DEFAULT_LOCALE, Some(report))?;
        changed = true;
    }

    if opts.download_external_resources {
        rt(report, "Downloading external resources");
        match download_resources(container, report) {
            Ok(did_change) => {
                if did_change {
                    changed = true;
                }
            }
            Err(e) => {
                report("Failed to download resources with error:");
                report(&format!("{e:?}"));
            }
        }
        report("");
    }

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::polish::container::{get_container, AnyContainer};
    use crate::oeb::polish::jacket::find_existing_jacket;
    use std::fs;

    fn write_container_xml(dir: &std::path::Path) {
        fs::create_dir_all(dir.join("META-INF")).unwrap();
        fs::write(
            dir.join("META-INF/container.xml"),
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
    }

    fn write_epub(dir: &std::path::Path) {
        write_container_xml(dir);
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Polish Test</dc:title>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><p>Some text -- with a dash and \"quotes\".</p></body></html>",
        )
        .unwrap();
    }

    /// `get_container` copies `dir` into a working temp directory and
    /// operates on the copy -- the returned `TempDir` guard must stay
    /// alive for as long as the container is used, or its backing
    /// files disappear out from under it.
    fn open_epub(dir: &std::path::Path) -> (AnyContainer, tempfile::TempDir) {
        let tdir = tempfile::tempdir().unwrap();
        let container = get_container(dir, tdir.path(), false).unwrap();
        (container, tdir)
    }

    #[test]
    fn no_actions_requested_makes_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let (mut c, _tdir) = open_epub(dir.path());
        let mut messages = Vec::new();
        let opts = PolishOptions::default();
        let changed = polish_one(&mut c, &opts, &mut |m: &str| messages.push(m.to_string()), None).unwrap();
        assert!(!changed);
    }

    #[test]
    fn jacket_and_smarten_punctuation_together_report_real_progress() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let (mut c, _tdir) = open_epub(dir.path());
        let mut messages = Vec::new();
        let opts = PolishOptions {
            jacket: true,
            smarten_punctuation: true,
            ..Default::default()
        };
        let changed = polish_one(&mut c, &opts, &mut |m: &str| messages.push(m.to_string()), None).unwrap();
        assert!(changed);
        let joined = messages.join("\n");
        assert!(joined.contains("Inserting metadata jacket"), "{joined}");
        assert!(joined.contains("Metadata jacket inserted"), "{joined}");
        assert!(joined.contains("Smartening punctuation"), "{joined}");

        assert!(find_existing_jacket(c.as_container_mut()).unwrap().is_some());
    }

    #[test]
    fn remove_jacket_reports_when_none_exists() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let (mut c, _tdir) = open_epub(dir.path());
        let mut messages = Vec::new();
        let opts = PolishOptions {
            remove_jacket: true,
            ..Default::default()
        };
        let changed = polish_one(&mut c, &opts, &mut |m: &str| messages.push(m.to_string()), None).unwrap();
        assert!(!changed);
        assert!(messages.iter().any(|m| m.contains("No metadata jacket found")), "{messages:?}");
    }

    #[test]
    fn replacing_metadata_from_an_opf_file_is_a_real_reported_error() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let (mut c, _tdir) = open_epub(dir.path());
        let opts = PolishOptions {
            opf: Some(std::path::PathBuf::from("/tmp/whatever.opf")),
            ..Default::default()
        };
        let err = polish_one(&mut c, &opts, &mut |_: &str| {}, None).unwrap_err();
        assert!(err.to_string().contains("not yet supported"), "{err}");
    }

    #[test]
    fn upgrade_on_an_already_epub3_book_reports_no_upgrade_needed() {
        let dir = tempfile::tempdir().unwrap();
        write_container_xml(dir.path());
        fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Already EPUB3</dc:title>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(dir.path().join("chap1.html"), b"<html><body><p>Hi</p></body></html>").unwrap();
        let (mut c, _tdir) = open_epub(dir.path());
        let mut messages = Vec::new();
        let opts = PolishOptions {
            upgrade_book: true,
            ..Default::default()
        };
        let changed = polish_one(&mut c, &opts, &mut |m: &str| messages.push(m.to_string()), None).unwrap();
        assert!(!changed);
        assert!(messages.iter().any(|m| m.contains("No upgrade needed")), "{messages:?}");
    }
}
