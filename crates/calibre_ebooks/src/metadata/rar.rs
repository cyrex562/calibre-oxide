//! Read metadata from RAR archives.
//!
//! Port of `src/calibre/ebooks/metadata/rar.py`: find the first
//! recognizable ebook inside a RAR archive (or, for comics, parse the
//! ComicBookInfo comment) and delegate to that format's metadata
//! reader.
//!
//! The Python accepts an open stream because calibre's own `unrar`
//! bindings can read from one; the `unrar` crate this port uses only
//! operates on filesystem paths, so [`get_metadata_from_path`] is the
//! primary entry point and [`get_metadata`] spools a stream to a temp
//! file first — the same shape `metadata/chm.rs` uses for `libchm`.

use crate::metadata::archive::{is_comic, parse_comic_comment};
use crate::metadata::MetaInformation;
use anyhow::{bail, Context, Result};
use calibre_utils::unrar;
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

/// The formats `rar.py` recognizes inside an archive, in the order the
/// Python's `set` happens to iterate (a `set` has no defined order in
/// CPython, but the archive's own file order is what actually decides
/// which one wins — this only decides which *extensions* are eligible).
const EBOOK_EXTS: [&str; 13] = [
    "lit", "opf", "prc", "mobi", "fb2", "epub", "rb", "imp", "pdf", "lrf", "azw", "azw1", "azw3",
];

/// `get_metadata` in `rar.py`, given a stream rather than a path.
///
/// The `unrar` crate needs a real file, so the stream is copied to a
/// temporary `.rar` file first.
pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let tmp = tempfile::Builder::new()
        .suffix(".rar")
        .tempfile()
        .context("rar: create tempfile")?;
    {
        let mut w = std::fs::File::create(tmp.path()).context("rar: open tempfile for write")?;
        std::io::copy(&mut stream, &mut w).context("rar: copy stream to tempfile")?;
        w.flush().ok();
    }
    get_metadata_from_path(tmp.path())
}

/// Direct-from-path variant. Preferred when the caller already has a
/// path on disk, which avoids the stream→tempfile copy.
pub fn get_metadata_from_path(path: &Path) -> Result<MetaInformation> {
    let file_names = unrar::names(path).map_err(|e| anyhow::anyhow!("rar: list entries: {e}"))?;

    if is_comic(&file_names) {
        // `rar.py` delegates to `calibre.ebooks.metadata.meta.get_metadata(stream, 'cbr')`,
        // which reads the ComicBookInfo JSON out of the archive
        // comment. The `unrar` crate (0.5.8) does not implement
        // comment retrieval yet — `Archive::set_comments` is a
        // documented no-op — so there is genuinely no comment to
        // read here. `parse_comic_comment` on an empty comment
        // returns a `MetaInformation::default()`, which is the same
        // result the Python gets from a comic with no CBI comment at
        // all, so this degrades gracefully rather than lying about
        // having read metadata.
        return parse_comic_comment(&[], "volume");
    }

    for name in &file_names {
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let Some(ext) = ext else { continue };
        if !EBOOK_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let data = unrar::extract_member(path, name)
            .map_err(|e| anyhow::anyhow!("rar: extract {name:?}: {e}"))?
            .with_context(|| format!("rar: {name:?} listed but could not be extracted"))?;
        let cursor = Cursor::new(data);
        let mut mi = dispatch(&ext, cursor, name)
            .with_context(|| format!("rar: reading metadata from {name:?}"))?;
        // `mi.timestamp = None` in the Python: a file's mtime inside
        // an archive isn't meaningful once extracted.
        mi.timestamp = None;
        return Ok(mi);
    }

    bail!("No ebook found in RAR archive")
}

/// `get_metadata(stream, stream_type)` for the one extension `rar.py`
/// found, minus the archive/RAR-recursion cases that can't occur here
/// (an ebook extension is never itself `rar` or `zip`).
fn dispatch(ext: &str, stream: Cursor<Vec<u8>>, name: &str) -> Result<MetaInformation> {
    match ext {
        "epub" => crate::metadata::epub::get_metadata(stream),
        "mobi" | "prc" | "azw" | "azw3" => crate::metadata::mobi::get_metadata(stream),
        "fb2" => crate::metadata::fb2::get_metadata(stream),
        "lit" => crate::metadata::lit::get_metadata(stream),
        "pdf" => crate::metadata::pdf::get_metadata(stream),
        "rb" => crate::metadata::rb::get_metadata(stream),
        "imp" => crate::metadata::imp::get_metadata(stream),
        "lrf" => crate::metadata::lrx::get_metadata(stream),
        // azw1 is calibre's Topaz format under an alias; see
        // `customize/builtins.py`'s `file_types = {'tpz', 'azw1'}`.
        "azw1" => crate::metadata::topaz::get_metadata(stream),
        "opf" => {
            let text = String::from_utf8_lossy(stream.get_ref()).into_owned();
            crate::opf::parse_opf(&text).map_err(|e| anyhow::anyhow!("{e}"))
        }
        other => bail!("rar: unhandled extension {other:?} for {name:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_a_stream_that_is_not_a_rar_archive() {
        let stream = Cursor::new(b"not a rar archive at all".to_vec());
        assert!(get_metadata(stream).is_err());
    }

    #[test]
    fn rejects_a_path_that_is_not_a_rar_archive() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        std::fs::write(tmp.path(), b"plain text, not RAR").expect("write");
        assert!(get_metadata_from_path(tmp.path()).is_err());
    }

    #[test]
    fn dispatch_rejects_an_extension_rar_never_recurses_into() {
        // `EBOOK_EXTS` never contains "rar" or "zip", so `dispatch`
        // should never see them, but the function itself should still
        // fail closed rather than silently succeeding if it ever does.
        let err = dispatch("rar", Cursor::new(Vec::new()), "nested.rar").unwrap_err();
        assert!(err.to_string().contains("unhandled extension"), "{err}");
    }

    #[test]
    fn dispatch_parses_an_embedded_opf() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uuid_id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>From RAR</dc:title>
    <dc:creator>An Author</dc:creator>
  </metadata>
</package>"#;
        let mi = dispatch("opf", Cursor::new(opf.to_vec()), "book.opf").expect("parses");
        assert_eq!(mi.title, "From RAR");
        assert_eq!(mi.authors, vec!["An Author".to_string()]);
    }

    #[test]
    fn azw1_is_dispatched_to_the_topaz_reader() {
        // Just confirms the extension is wired to a reader that
        // returns *some* result rather than falling into the
        // catch-all `unhandled extension` branch; topaz's own parsing
        // correctness is covered by `metadata::topaz`'s own tests.
        let err =
            dispatch("azw1", Cursor::new(b"not a topaz file".to_vec()), "b.azw1").unwrap_err();
        assert!(
            !err.to_string().contains("unhandled extension"),
            "azw1 should reach the topaz reader: {err}"
        );
    }
}
