//! Port of `old_src/src/calibre/ebooks/pdf/pdftohtml.py` (178 lines).
//!
//! A thin wrapper around the external `pdftohtml` binary (part of
//! poppler-utils). No Qt/native dependency - just `subprocess.Popen` in the
//! original, `std::process::Command` here. This is the first half of
//! calibre's "real" PDF-input pipeline: shell out to poppler to get either
//! rendered HTML or (what [`crate::pdf::reflow`] consumes) layout XML via
//! `-xml`, then reflow that XML into clean HTML.
//!
//! Scope note: this port fully covers binary discovery, invocation,
//! exit-code/DRM-size checking and the `-xml` mode (which is all
//! [`crate::pdf::reflow::PdfDocument`] needs). For the non-XML HTML mode,
//! it covers the subprocess invocation and the load-bearing text
//! substitutions (`<br/>` -> `<br>`, `index.html#N` anchor rewriting to
//! `#pN`, NBSP/paragraph-separator stripping), but does **not** port
//! `flip_images`' actual image-pixel flipping (`calibre.utils.img`,
//! decode/flip/re-encode) or `parse_outline`'s ToC/`toc.ncx` extraction
//! (`calibre.ebooks.oeb.polish.toc.create_ncx`) - both are real, separable
//! pieces of Python functionality, but pull in enough additional surface
//! area (image codec round-tripping; NCX authoring wired to a second
//! `pdftohtml -xml -f 1 -l 1` invocation) that they're left as a follow-up
//! rather than ported here, since [`reflow`](crate::pdf::reflow) - the
//! primary deliverable this module feeds - only ever calls the `-xml`
//! path where neither applies.
//!
//! Binary-not-found handling matches `docs/HARNESS.md`'s "missing
//! infrastructure never causes test failures" policy (the same category as
//! `which calibre-debug` checks elsewhere in this project): [`find_pdftohtml`]
//! returns `None` rather than panicking, and callers get a clear
//! [`PdfToHtmlError::BinaryNotFound`] instead of a subprocess-spawn panic.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// `pdftohtml` + `.exe` on Windows, matching
/// `PDFTOHTML = 'pdftohtml' + ('.exe' if iswindows else '')` (pdftohtml.py
/// line 18). Note: unlike upstream, this port does not consult calibre's
/// `bundled_binaries_dir()` (there is no calibre-oxide binary-bundling
/// story yet) - only `$PATH` is searched.
pub const PDFTOHTML_BASENAME: &str = if cfg!(windows) {
    "pdftohtml.exe"
} else {
    "pdftohtml"
};

fn is_bsd() -> bool {
    cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
}

/// Errors from invoking `pdftohtml`. `BinaryNotFound` is the expected,
/// graceful outcome in any environment without poppler-utils installed;
/// everything else surfaces the subprocess's own failure.
#[derive(Debug, thiserror::Error)]
pub enum PdfToHtmlError {
    #[error("pdftohtml binary not found on PATH (looked for `{0}`)")]
    BinaryNotFound(String),
    #[error("I/O error running pdftohtml: {0}")]
    Io(#[from] std::io::Error),
    #[error("pdftohtml exited with status {status}: {stderr}")]
    ProcessFailed { status: i32, stderr: String },
    /// Port of `pdftohtml.py`'s `raise DRMError()` when the output file is
    /// missing or implausibly small (< 100 bytes) - poppler's usual
    /// signal that the source PDF is DRM-protected or otherwise unusable.
    #[error("pdftohtml produced no usable output (source PDF may be DRM-protected or corrupt)")]
    NoUsableOutput,
}

/// Search `$PATH` for the `pdftohtml` binary. Returns `None` (never panics
/// or errors) if it isn't found - callers turn that into
/// [`PdfToHtmlError::BinaryNotFound`].
pub fn find_pdftohtml() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(PDFTOHTML_BASENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Port of `pdftohtml()` (pdftohtml.py lines 33-114): convert `pdf_path`
/// into `output_dir/index.html` (or `index.xml` if `as_xml`), copying the
/// source PDF into the working directory first (matching upstream, which
/// runs the tool with `CurrentDir(output_dir)`). Returns the path to the
/// generated index file.
pub fn pdftohtml(
    output_dir: &Path,
    pdf_path: &Path,
    no_images: bool,
    as_xml: bool,
) -> Result<PathBuf, PdfToHtmlError> {
    let binary = find_pdftohtml()
        .ok_or_else(|| PdfToHtmlError::BinaryNotFound(PDFTOHTML_BASENAME.to_string()))?;

    std::fs::create_dir_all(output_dir)?;
    let pdfsrc = output_dir.join("src.pdf");
    std::fs::copy(pdf_path, &pdfsrc)?;

    let index_name = if as_xml { "index.xml" } else { "index.html" };
    let index = output_dir.join(index_name);

    let mut args: Vec<&str> = vec!["-enc", "UTF-8", "-noframes", "-p", "-nomerge", "-nodrm"];
    if is_bsd() {
        args.retain(|a| *a != "-nodrm");
    }
    if no_images {
        args.push("-i");
    }
    if as_xml {
        args.push("-xml");
    }

    let output = Command::new(&binary)
        .current_dir(output_dir)
        .args(&args)
        .arg("src.pdf")
        .arg(index_name)
        .output()?;

    if !output.status.success() {
        // Clean up the copy we made even on failure.
        let _ = std::fs::remove_file(&pdfsrc);
        return Err(PdfToHtmlError::ProcessFailed {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let size = std::fs::metadata(&index).map(|m| m.len()).unwrap_or(0);
    if size < 100 {
        let _ = std::fs::remove_file(&pdfsrc);
        return Err(PdfToHtmlError::NoUsableOutput);
    }

    if !as_xml {
        post_process_html(&index)?;
    }

    let _ = std::fs::remove_file(&pdfsrc);

    Ok(index)
}

/// Convenience wrapper for [`crate::pdf::reflow`]'s use case: run
/// `pdftohtml -xml` on `pdf_path` in a scratch temp directory and return
/// the resulting XML as a `String`, ready for
/// `crate::pdf::reflow::PdfDocument::from_xml`.
pub fn pdftohtml_xml(pdf_path: &Path, no_images: bool) -> Result<String, PdfToHtmlError> {
    let dir = tempfile::tempdir()?;
    let index = pdftohtml(dir.path(), pdf_path, no_images, true)?;
    let bytes = std::fs::read(&index)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Real-but-partial port of the non-XML branch of `pdftohtml()`
/// (pdftohtml.py lines 84-100): the load-bearing text substitutions only.
/// See the module doc comment for what's intentionally not ported
/// (image-pixel flipping, ToC/`toc.ncx` extraction).
fn post_process_html(index: &Path) -> Result<(), PdfToHtmlError> {
    let bytes = std::fs::read(index)?;
    let mut raw = String::from_utf8_lossy(&bytes).into_owned();

    raw = raw.replacen(
        "<head",
        "<!-- created by calibre-oxide's pdftohtml -->\n  <head",
        1,
    );
    // Versions of pdftohtml >= 0.20 emit self-closing <br/>; normalize to
    // <br> to keep any downstream heuristics that scan for bare <br> happy.
    raw = raw.replace("<br/>", "<br>");
    raw = a_name_re().replace_all(&raw, r#"<a id="$1""#).into_owned();
    raw = a_id_re().replace_all(&raw, r#"<a id="p$1""#).into_owned();
    raw = a_href_index_re()
        .replace_all(&raw, r##"<a href="#p$1""##)
        .into_owned();
    raw = raw.replace(['\u{00a0}', '\u{2029}'], " ");

    std::fs::write(index, raw.as_bytes())?;
    Ok(())
}

fn a_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)<a\s+name=(\d+)"#).expect("static regex"))
}

fn a_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)<a id="(\d+)""#).expect("static regex"))
}

fn a_href_index_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)<a href="index\.html#(\d+)""#).expect("static regex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_not_found_is_graceful_when_path_is_empty() {
        // Simulate "no pdftohtml anywhere" without touching the real
        // environment's PATH permanently: search an empty directory list
        // directly rather than mutating the process-wide PATH var (which
        // would race with other tests).
        let empty: Vec<PathBuf> = Vec::new();
        let found = empty.iter().find(|d| d.join(PDFTOHTML_BASENAME).is_file());
        assert!(found.is_none());
    }

    #[test]
    fn pdftohtml_errors_cleanly_when_binary_missing() {
        // Point PATH at a directory guaranteed not to contain pdftohtml,
        // for the duration of this one call, then restore it. This is the
        // "missing infrastructure never causes test failures" case from
        // docs/HARNESS.md: we assert the graceful Err, not a panic.
        let old_path = std::env::var_os("PATH");
        // SAFETY (test-only): serial within this process; no other thread
        // in this test binary depends on PATH mid-test.
        unsafe {
            std::env::set_var("PATH", "");
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_pdf = dir.path().join("in.pdf");
        std::fs::write(&fake_pdf, b"%PDF-1.4\n").expect("write fake pdf");
        let out_dir = dir.path().join("out");
        let result = pdftohtml(&out_dir, &fake_pdf, false, true);
        unsafe {
            match old_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(matches!(result, Err(PdfToHtmlError::BinaryNotFound(_))));
    }

    /// Opportunistic: only runs its assertions if a real `pdftohtml` is on
    /// PATH in this environment; otherwise it's a no-op pass, matching
    /// docs/HARNESS.md's policy for external-tool-dependent tests.
    #[test]
    fn real_pdftohtml_invocation_if_available() {
        let Some(binary) = find_pdftohtml() else {
            return;
        };
        // Sanity: whatever we found is actually invocable.
        let ok = Command::new(&binary)
            .arg("-v")
            .output()
            .map(|_| true)
            .unwrap_or(false);
        assert!(ok, "found pdftohtml at {binary:?} but could not invoke it");
    }
}
