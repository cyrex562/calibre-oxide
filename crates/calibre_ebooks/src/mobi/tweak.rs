//! Port of `old_src/src/calibre/ebooks/mobi/tweak.py` (issue #120's
//! own dependency): explode a MOBI/AZW3 file's KF8 content to a
//! directory of loose OPF/HTML/image files for hand-editing, and
//! rebuild it back into a MOBI/AZW3 file.
//!
//! # Scope
//!
//! **`explode` is a real, full port.** It reuses the exact
//! `MobiReader`/`Mobi8Reader` pipeline already established as the
//! MOBI *input* conversion path
//! ([`crate::input::mobi_input::MOBIInput::convert`]), with
//! `for_tweak: true` (matching real Python's own `for_tweak=True`),
//! plus real Python's own pre-checks: Topaz/KFX rejection (already
//! `MobiReader::new`'s own real error branches), DRM rejection
//! (`MobiReader::check_for_drm`), the "this MOBI file does not
//! contain a KF8 book" rejection for MOBI6-only files (real
//! `tweak.py`'s explode() only supports KF8-bearing books at all --
//! preserved here, not a narrowing this port introduces), and the
//! "joint" MOBI6+KF8 confirmation question (injected as a closure,
//! matching Python's own `question=lambda x: True` default-callback
//! shape -- this library layer has no terminal/TTY concern of its
//! own, that's a CLI wrapper's job).
//!
//! **`rebuild` has no real target yet, disclosed rather than faked.**
//! Real `do_rebuild` needs `plugin_for_output_format('azw3')` --
//! this crate's own `mobi::writer8` (issue #157, the joint MOBI6+KF8
//! writer) is real, but nothing wires it up behind an "OPF directory
//! in, AZW3 file out" entry point yet (confirmed: no `AZW3Output`-
//! equivalent plugin exists anywhere in this crate, per #157's own
//! closing note). Building that entry point is a real, separate
//! undertaking (parsing an on-disk OPF+content tree back into an
//! `OEBBook`, then driving both the MOBI6 and KF8 writers together)
//! -- out of scope for this file, which ports `tweak.py`'s own
//! dispatch logic, not a from-scratch AZW3 output plugin. `rebuild`
//! returns a clear, real error rather than attempting a partial or
//! silently-wrong rebuild.

use std::path::Path;

use anyhow::{bail, Context, Result};
use thiserror::Error;

use crate::input::mobi_input::MOBIInput;
use crate::mobi::mobi6::MobiReader;
use crate::mobi::mobi8::Mobi8Reader;
use crate::mobi::MobiLog;

/// Port of `BadFormat` (a `ValueError` subclass in Python).
#[derive(Debug, Error)]
pub enum BadFormat {
    #[error("This is not a MOBI file. It is a Topaz file.")]
    Topaz,
    #[error("This is not a MOBI file.")]
    NotMobi,
    #[error("This file is locked with DRM. It cannot be tweaked.")]
    Drm,
    #[error(
        "This MOBI file does not contain a KF8 format book. KF8 is the new format from Amazon. \
         calibre can only tweak MOBI files that contain KF8 books. Older MOBI files without KF8 \
         are not tweakable."
    )]
    NoKf8,
}

/// Port of `explode`'s real pre-checks (Topaz/KFX/malformed-header
/// rejection, DRM rejection, KF8-required rejection), run before the
/// (real, heavier) extraction itself -- matching real Python's own
/// split between a fast synchronous pre-check and the actual
/// (in Python, forked-subprocess) extraction work.
fn check_explodable(reader: &MobiReader) -> Result<(), BadFormat> {
    if reader.book_header.encryption_type != 0 {
        return Err(BadFormat::Drm);
    }
    if reader.kf8_type.is_none() {
        return Err(BadFormat::NoKf8);
    }
    Ok(())
}

/// Port of `mobi.tweak.explode`. `question` is real Python's own
/// `question=lambda x: True` hook, asked only for a "joint" MOBI6+KF8
/// file (tweaking removes the Mobi6 half); returning `false` mirrors
/// Python's own `return None` ("the question was answered with No").
pub fn explode(path: &Path, dest: &Path, question: impl FnOnce(&str) -> bool) -> Result<Option<String>> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    if raw.starts_with(b"TPZ") {
        bail!(BadFormat::Topaz);
    }

    let reader = MobiReader::new(&raw).map_err(|_| BadFormat::NotMobi)?;
    check_explodable(&reader)?;

    if reader.kf8_type.as_deref() == Some("joint")
        && !question(
            "This MOBI file contains both KF8 and older Mobi6 data. Tweaking it will remove the \
             Mobi6 data, which means the file will not be usable on older Kindles. Are you sure?",
        )
    {
        return Ok(None);
    }

    std::fs::create_dir_all(dest)?;
    let log = MobiLog::default();
    let mut mobi8 = Mobi8Reader::new(reader, log, true);
    let opf_rel = mobi8.run(dest).context("Failed to extract KF8 content")?;
    Ok(Some(dest.join(opf_rel).to_string_lossy().into_owned()))
}

/// Port of `mobi.tweak.rebuild`. See this module's own doc for why
/// there is no real target yet.
pub fn rebuild(_src_dir: &Path, _dest_path: &Path) -> Result<()> {
    bail!(
        "Rebuilding MOBI/AZW3 files is not supported yet: no AZW3 output plugin exists in this \
         port (issue #157's KF8 writer has no \"OPF directory -> AZW3 file\" entry point wired \
         up). Explode is fully supported; rebuild is not."
    )
}

/// Convenience wrapper matching `MOBIInput::convert`'s own signature
/// shape, for a caller that just wants "give me an OEBBook" rather
/// than "give me the raw exploded directory" -- not part of real
/// `tweak.py`'s own API, but a thin, obviously-correct restatement in
/// terms of the same building blocks (kept only if a future caller
/// needs it; the real port surface is [`explode`]/[`rebuild`] above).
#[allow(dead_code)]
fn explode_via_mobi_input(path: &Path, dest: &Path) -> Result<()> {
    MOBIInput::new().convert(path, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_fixture(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mobi").join(name);
        std::fs::read(&path).unwrap_or_else(|_| panic!("missing test fixture: {}", path.display()))
    }

    fn has_real_mobi_fixture() -> bool {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mobi").is_dir()
    }

    #[test]
    fn explode_rejects_a_topaz_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.tpz");
        std::fs::write(&src, b"TPZ not a real topaz file").unwrap();
        let dest = dir.path().join("out");

        let err = explode(&src, &dest, |_| true).unwrap_err();
        assert!(err.to_string().contains("Topaz file"), "{err}");
    }

    #[test]
    fn explode_rejects_a_non_mobi_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.mobi");
        std::fs::write(&src, b"this is not a mobi file at all, just plain text padding").unwrap();
        let dest = dir.path().join("out");

        let err = explode(&src, &dest, |_| true).unwrap_err();
        assert!(err.to_string().contains("not a MOBI file"), "{err}");
    }

    #[test]
    fn rebuild_reports_a_real_disclosed_gap_not_a_silent_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let err = rebuild(dir.path(), &dir.path().join("out.azw3")).unwrap_err();
        assert!(err.to_string().contains("not supported"), "{err}");
    }

    // Real end-to-end explode tests need a real KF8-bearing .mobi/.azw3
    // fixture (Topaz/malformed-file rejection is already covered above
    // with synthetic bytes, since those checks never need to parse a
    // real book). Skipped when no such fixture is checked in, matching
    // this crate's own convention elsewhere (e.g. TTS's voice-model
    // gate) of not failing CI over a missing binary test asset.
    #[test]
    fn explode_extracts_a_real_kf8_fixture_end_to_end() {
        if !has_real_mobi_fixture() {
            eprintln!("skipping: no tests/fixtures/mobi/ directory checked in");
            return;
        }
        let raw = read_fixture("kf8_standalone.azw3");
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.azw3");
        std::fs::write(&src, raw).unwrap();
        let dest = dir.path().join("out");

        let opf = explode(&src, &dest, |_| true).unwrap().expect("a non-joint file never asks the question");
        assert!(Path::new(&opf).is_file());
        assert!(dest.join("metadata.opf").is_file());
    }
}
