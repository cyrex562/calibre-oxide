//! Top-level `inspect_mobi` entry point.
//!
//! Port of `src/calibre/ebooks/mobi/debug/main.py`: parse a MOBI file
//! once, then route to the MOBI6 or MOBI8 dumper (or both, for a
//! "joint" file carrying both a MOBI6 and a KF8 rendition) based on
//! what [`super::headers::MobiFile::kf8_type`] reports.

use std::path::Path;

use anyhow::{Context, Result};

use super::headers::MobiFile;
use super::{mobi6, mobi8};

/// `inspect_mobi` in the Python. `ddir` must already exist (and,
/// unlike the Python, is never created or cleared here — the Python's
/// `shutil.rmtree`-then-`os.makedirs` dance is a CLI convenience this
/// library function leaves to its caller, since silently deleting a
/// caller-supplied directory is not something a library should do on
/// their behalf).
pub fn inspect_mobi(raw: Vec<u8>, ddir: &Path) -> Result<()> {
    let f = MobiFile::parse(raw).context("parsing MOBI file")?;
    match f.kf8_type {
        None => mobi6::inspect_mobi(f, ddir),
        Some("joint") => {
            let p6 = ddir.join("mobi6");
            let p8 = ddir.join("mobi8");
            std::fs::create_dir_all(&p6)?;
            std::fs::create_dir_all(&p8)?;
            // The MOBI8 dumper reads `f.mobi8_header`/`f.decompress8`
            // off the same parsed file, so both halves are dumped from
            // one parse, exactly as `main.py` does (it passes the same
            // `f` to both `inspect_mobi6` and `inspect_mobi8`).
            let raw_for_mobi8 = f.raw.clone();
            mobi6::inspect_mobi(f, &p6)?;
            let f8 = MobiFile::parse(raw_for_mobi8)?;
            mobi8::inspect_mobi(f8, &p8)
        }
        Some(_) => mobi8::inspect_mobi(f, ddir),
    }
}

/// Read `path`, parse it as MOBI, and dump into `ddir` (created if
/// missing). The convenience wrapper `main()` builds on in the Python.
pub fn inspect_mobi_file(path: &Path, ddir: &Path) -> Result<()> {
    let raw = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
    std::fs::create_dir_all(ddir)?;
    inspect_mobi(raw, ddir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_mobi_rejects_a_non_mobi_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = inspect_mobi(b"not a mobi file".to_vec(), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("parsing MOBI file"), "{err}");
    }

    #[test]
    fn inspect_mobi_file_reads_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("book.mobi");
        std::fs::write(&src, b"garbage").expect("write");
        let ddir = tmp.path().join("out");
        let err = inspect_mobi_file(&src, &ddir).unwrap_err();
        // Reaches the parse stage (and fails there), proving the file
        // was actually read and the output directory got created.
        assert!(ddir.exists());
        assert!(err.to_string().contains("parsing MOBI file"), "{err}");
    }
}
