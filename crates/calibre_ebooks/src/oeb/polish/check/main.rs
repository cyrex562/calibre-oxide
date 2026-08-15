//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/main.py`.
//!
//! # Scope note: `book_type`
//!
//! Like [`super::opf::check_opf`]/[`super::links::check_link_destinations`],
//! [`run_checks`]/[`fix_errors`] take `book_type` as an explicit
//! parameter rather than reading `container.book_type()` -- see
//! `check::opf`'s module docs for why (Rust's static method dispatch
//! means a function written against `&mut Container` would always see
//! `"oeb"`, even when called through `EpubContainer`'s `Deref`).
//!
//! # Scope note: the CSS-linting steps
//!
//! Python's `run_checks` has two CSS-linting steps (whole stylesheets,
//! then inline `<style>`/`style=""` content), both going through
//! `check/css.py`'s `CSSChecker`, which needs the stylelint/WebEngine
//! infrastructure documented as a real gap in [`super::css`]'s module
//! docs. Calling into that gap here would turn "run the checker on an
//! ordinary book with any CSS at all" into a guaranteed panic, which
//! would defeat the entire point of having every *other* checker be
//! real. This port skips the CSS-linting steps specifically (still
//! flagging empty stylesheets, which is real, gap-free logic) while
//! running every other checker for real -- see the loop below for
//! exactly which two Python steps are skipped and why.

use anyhow::Result;

use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::cover::is_raster_image;

use super::super::container::Container;
use super::base::{run_checkers, CheckError, Level};
use super::css::{self, Job};
use super::fonts::check_fonts;
use super::images::check_raster_images;
use super::links::{check_link_destinations, check_links, check_mimetypes};
use super::opf::check_opf;
use super::parsing::{
    check_encoding_declarations, check_filenames, check_ids, check_markup, check_xml_parsing,
    empty_file, xml_types,
};

/// Port of `Container.MAX_HTML_FILE_SIZE`'s per-subclass values (`0` on
/// the base `Container`, `260 * 1024` on `EpubContainer`, `512 * 1024`
/// on `KEPUBContainer`) -- see the module docs for why this is a
/// function of an explicit `book_type` rather than a container method.
pub fn max_html_file_size(book_type: &str) -> u64 {
    match book_type {
        "epub" => 260 * 1024,
        "kepub" => 512 * 1024,
        _ => 0,
    }
}

/// Port of `CSSChecker`. Not called by [`run_checks`] (see the module
/// docs) -- kept for API parity and for any caller that wants to invoke
/// the documented `check_css` gap directly.
pub struct CssChecker {
    jobs: Vec<Job>,
}

impl Default for CssChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl CssChecker {
    pub fn new() -> Self {
        CssChecker { jobs: Vec::new() }
    }

    pub fn create_job(&mut self, name: &str, raw: &str, line_offset: i64, is_declaration: bool) {
        self.jobs
            .push(css::create_job(name, raw, line_offset, is_declaration));
    }

    /// Port of `CSSChecker.__call__`. `todo!()`s (via [`css::check_css`])
    /// if any jobs were queued -- see [`super::css`]'s module docs.
    pub fn run(&self) -> Vec<CheckError> {
        if self.jobs.is_empty() {
            return Vec::new();
        }
        css::check_css(&self.jobs)
    }
}

type RawItem = (String, String, Vec<u8>);

/// Port of `run_checks`.
pub fn run_checks(container: &mut Container, book_type: &str) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();

    let mut xml_items: Vec<RawItem> = Vec::new();
    let mut html_items: Vec<RawItem> = Vec::new();
    let mut raster_images: Vec<RawItem> = Vec::new();
    let mut stylesheets: Vec<RawItem> = Vec::new();

    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();
    let xtypes = xml_types();

    for (name, mt) in &names {
        if xtypes.contains(mt) {
            let raw = container.raw_data(name, false)?;
            xml_items.push((name.clone(), mt.clone(), raw));
        } else if OEB_DOCS.contains(&mt.as_str()) {
            let raw = container.raw_data(name, false)?;
            html_items.push((name.clone(), mt.clone(), raw));
        } else if OEB_STYLES.contains(&mt.as_str()) {
            let raw = container.raw_data(name, true)?;
            stylesheets.push((name.clone(), mt.clone(), raw));
        } else if is_raster_image(Some(mt.as_str())) {
            let raw = container.raw_data(name, false)?;
            raster_images.push((name.clone(), mt.clone(), raw));
        }
    }

    let max_size = max_html_file_size(book_type);
    if max_size > 0 {
        errors.extend(run_checkers(&html_items, |item| {
            super::parsing::check_html_size(&item.0, &item.2, max_size)
        }));
    }
    errors.extend(run_checkers(&xml_items, |item| {
        check_xml_parsing(&item.0, &item.1, &item.2)
    }));
    errors.extend(run_checkers(&html_items, |item| {
        check_xml_parsing(&item.0, &item.1, &item.2)
    }));
    errors.extend(run_checkers(&raster_images, |item| {
        check_raster_images(&item.0, &item.2)
    }));

    if errors.iter().any(|e| e.level > Level::Warn) {
        return Ok(errors);
    }

    // Python step 1/2 of CSS linting ("css uses its own worker pool"):
    // whole stylesheets. Skipped -- see the module docs. Flagging empty
    // stylesheets is real, gap-free logic and still runs.
    for (name, _mt, raw) in &stylesheets {
        if raw.is_empty() {
            errors.push(empty_file(name));
        }
    }

    for (name, _mt, raw) in html_items.iter().chain(xml_items.iter()) {
        errors.extend(check_encoding_declarations(name, raw));
    }

    // Python step 2/2 of CSS linting: inline `<style>` tags and
    // `style=""` attributes inside content documents. Skipped for the
    // same reason as above.

    errors.extend(check_mimetypes(container)?);
    errors.extend(check_links(container)?);
    errors.extend(check_link_destinations(container, book_type)?);
    errors.extend(check_fonts(container)?);
    errors.extend(check_ids(container)?);
    errors.extend(check_filenames(container));
    errors.extend(check_markup(container)?);
    errors.extend(check_opf(container, book_type)?);

    Ok(errors)
}

/// Port of `fix_css`. Needs the same stylelint/WebEngine infrastructure
/// as [`css::check_css`] (running with `fix: true`) -- see
/// [`super::css`]'s module docs. Unreachable through [`fix_errors`] in
/// practice, since [`run_checks`] never produces a `fixable_css_error`
/// (the linting step that would is skipped above) -- kept `todo!()`,
/// real-signature, for any caller that constructs a `fixable_css_error`
/// by hand.
pub fn fix_css(_container: &mut Container) -> Result<bool> {
    todo!(
        "placeholder: fixing CSS needs the same stylelint/QWebEnginePage infrastructure as \
         check_css -- see oeb::polish::check::css's module docs"
    )
}

/// Port of `fix_errors`.
pub fn fix_errors(container: &mut Container, mut errors: Vec<CheckError>) -> Result<bool> {
    let mut changed = false;

    let mut parsing_error_names: Vec<String> = errors
        .iter()
        .filter(|e| e.is_parsing_error)
        .map(|e| e.name.clone())
        .collect();
    parsing_error_names.sort();
    parsing_error_names.dedup();
    for name in parsing_error_names {
        if container.ensure_parsed(&name).is_err() {
            continue;
        }
        container.dirty(&name);
        // Python additionally re-pretty-prints any non-empty `<style>`
        // tag in this document (`fix_style_tag`/`pretty_script_or_style`).
        // Intentionally skipped -- see check::fonts's module docs: that
        // function's CSS-reformatting path is a pre-existing, documented
        // todo!() upstream in oeb::polish::pretty, and calling it on real
        // content is a guaranteed panic for what is a purely cosmetic
        // step (re-indentation), not a correctness one.
        changed = true;
    }

    let mut has_fixable_css_errors = false;
    for err in &mut errors {
        if err.fixable_css_error {
            has_fixable_css_errors = true;
        }
        if err.is_fixable() && err.apply_fix(container)? {
            changed = true;
        }
    }
    if has_fixable_css_errors && fix_css(container)? {
        changed = true;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_container(dir: &Path, opf: &str, files: &[(&str, &[u8])]) -> Container {
        std::fs::write(dir.join("content.opf"), opf).unwrap();
        for (name, data) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, data).unwrap();
        }
        Container::open(dir, &dir.join("content.opf")).unwrap()
    }

    const GOOD_OPF: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test</dc:title>
    <dc:identifier id="bookid">urn:uuid:x</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

    #[test]
    fn run_checks_accepts_a_clean_book() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            GOOD_OPF,
            &[(
                "chap1.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>hi</p></body></html>",
            )],
        );
        let errors = run_checks(&mut c, "epub").unwrap();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn run_checks_finds_duplicate_ids_and_dangling_links() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            GOOD_OPF,
            &[(
                "chap1.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p id=\"a\">1</p><p id=\"a\">2</p><a href=\"missing.html\">x</a></body></html>",
            )],
        );
        let errors = run_checks(&mut c, "epub").unwrap();
        assert!(errors.iter().any(|e| e.type_name == "DuplicateId"));
        assert!(errors.iter().any(|e| e.type_name == "DanglingLink"));
    }

    #[test]
    fn run_checks_short_circuits_on_severe_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), GOOD_OPF, &[("chap1.html", b"")]);
        let errors = run_checks(&mut c, "epub").unwrap();
        // An EmptyFile is ERROR level; run_checks should return early,
        // before check_opf/check_links etc. run.
        assert!(errors.iter().any(|e| e.type_name == "EmptyFile"));
        assert!(!errors.iter().any(|e| e.type_name == "NoUID"));
    }

    #[test]
    fn fix_errors_applies_individual_fixes_and_reports_changed() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            GOOD_OPF,
            &[(
                "chap1.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p id=\"a\">1</p><p id=\"a\">2</p></body></html>",
            )],
        );
        let errors = run_checks(&mut c, "epub").unwrap();
        assert!(errors.iter().any(|e| e.type_name == "DuplicateId"));
        let changed = fix_errors(&mut c, errors).unwrap();
        assert!(changed);
        let errors_after = run_checks(&mut c, "epub").unwrap();
        assert!(!errors_after.iter().any(|e| e.type_name == "DuplicateId"));
    }

    #[test]
    fn max_html_file_size_matches_python_constants() {
        assert_eq!(max_html_file_size("epub"), 260 * 1024);
        assert_eq!(max_html_file_size("kepub"), 512 * 1024);
        assert_eq!(max_html_file_size("oeb"), 0);
    }
}
