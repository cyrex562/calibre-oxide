//! Integration test for the RAR metadata reader against a real archive.
//!
//! `data/rar_gitignore_only.rar` is a genuine RAR v5 archive (vendored,
//! MIT/Apache-2.0, from the `unrar` crate's own test corpus — see
//! `unrar-0.5.8/data/comment.rar`) containing a single `.gitignore`
//! entry. That extension is neither an ebook format nor a comic image,
//! so this exercises real RAR-container parsing — magic bytes, header
//! decoding, entry listing — end to end, distinct from the
//! synthetic-bytes error-path tests in `metadata::rar`'s own unit
//! tests.

use calibre_ebooks::metadata::rar::get_metadata_from_path;
use std::path::Path;

#[test]
fn a_real_rar_archive_with_no_ebook_inside_reports_none_found() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/rar_gitignore_only.rar");
    let err = get_metadata_from_path(&path).unwrap_err();
    assert!(
        err.to_string().contains("No ebook found in RAR archive"),
        "{err}"
    );
}
