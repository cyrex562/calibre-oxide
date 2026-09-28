//! Black-box tests of `calibredb backup_metadata` / `restore_database`
//! and of per-book identity (#949, #950).
//!
//! These run the compiled binary against a real library, which is what
//! made the defects visible: every one of them was invisible to the
//! in-process unit tests, because those construct the pre-#889 library
//! layout by hand.

use std::io::Write;
use std::process::Command;

use calibre_oxide_e2e::{oxide_bin, TestLibrary};

fn run(library: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(oxide_bin("calibredb"))
        .args(args)
        .arg("--with-library")
        .arg(library)
        .output()
        .expect("failed to run the compiled calibredb binary")
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A real EPUB carrying `uuid` as its `dc:identifier` and `cover` bytes
/// as its cover image.
fn write_epub(path: &std::path::Path, title: &str, uuid: &str, cover: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    zip.start_file(
        "mimetype",
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    let deflated = zip::write::FileOptions::default();
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();

    zip.start_file("content.opf", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title><dc:identifier id="uid">urn:uuid:{uuid}</dc:identifier><meta name="cover" content="cov"/></metadata><manifest><item id="cov" href="cover.jpg" media-type="image/jpeg"/><item id="c1" href="c1.html" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
        )
        .as_bytes(),
    )
    .unwrap();

    zip.start_file("cover.jpg", deflated).unwrap();
    zip.write_all(cover).unwrap();
    zip.start_file("c1.html", deflated).unwrap();
    zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Text.</p></body></html>"#).unwrap();
    zip.finish().unwrap();
}

/// A minimal real JPEG, `filler` bytes long, so two covers are
/// distinguishable by size alone.
fn jpeg(filler: usize) -> Vec<u8> {
    [&[0xFFu8, 0xD8, 0xFF, 0xE0][..], &vec![0xAA; filler][..], &[0xFF, 0xD9][..]].concat()
}

/// #950: two books whose files happen to carry the same `urn:uuid:`
/// identifier must not share one cover file.
///
/// `books.uuid` is the cover sidecar's filename, and it used to be taken
/// straight from the book file, so the second book's cover overwrote the
/// first's and both books then displayed the same picture. Adding the
/// same book twice was enough to trigger it.
#[test]
fn two_books_sharing_an_identifier_keep_their_own_covers() {
    let lib = TestLibrary::new();
    let (small, large) = (jpeg(200), jpeg(400));

    for (title, cover) in [("One", &small), ("Two", &large)] {
        let path = lib.path().join(format!("Book {title}.epub"));
        write_epub(&path, title, "an-identifier-two-files-share", cover);
        assert!(run(lib.path(), &["add", path.to_str().unwrap()]).status.success());
    }

    let library = calibre_db::Library::open(lib.path().to_path_buf()).unwrap();
    let cache = library.as_cache();

    let first = calibre_db::covers::cover_path(&cache, 1).unwrap();
    let second = calibre_db::covers::cover_path(&cache, 2).unwrap();
    assert_ne!(first, second, "both books resolved to one cover file");

    // Each book kept the cover that was embedded in its own file.
    assert_eq!(std::fs::metadata(&first).unwrap().len() as usize, small.len());
    assert_eq!(std::fs::metadata(&second).unwrap().len() as usize, large.len());

    // The identifier from the file is kept, just not as the library's
    // identity for the row.
    assert_ne!(
        cache.field_for(1, "uuid").unwrap(),
        cache.field_for(2, "uuid").unwrap(),
        "two books were given the same library uuid"
    );
}

/// #949: `backup_metadata --all` used to print `Backup complete.` having
/// written nothing at all, because it skipped every book whose `path` was
/// empty -- which since #889 is every book.
#[test]
fn backing_up_metadata_writes_a_sidecar_per_book_and_says_how_many() {
    let lib = TestLibrary::new();
    for title in ["One", "Two"] {
        let path = lib.path().join(format!("Book {title}.epub"));
        write_epub(&path, title, &format!("identifier-{title}"), &jpeg(100));
        assert!(run(lib.path(), &["add", path.to_str().unwrap()]).status.success());
    }

    let out = run(lib.path(), &["backup_metadata", "--all"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout_of(&out);
    assert!(text.contains("Backed up 2 books"), "the summary must count what was written, got: {text}");

    let sidecars: Vec<_> = std::fs::read_dir(lib.path().join(".calibre-oxide/opf"))
        .expect("no OPF sidecar directory was created")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(sidecars.len(), 2, "one OPF per book, got {sidecars:?}");
}

/// #949: `restore_database` renames `metadata.db` away before looking for
/// anything to rebuild from, so a library with no OPFs was left with an
/// empty database. That was unreachable while `--really-do-it` could not
/// be parsed; now that it can be, the command must refuse instead.
#[test]
fn restoring_with_nothing_to_restore_from_leaves_the_database_alone() {
    let lib = TestLibrary::new();
    let path = lib.path().join("A Book.epub");
    write_epub(&path, "Only Book", "identifier-only", &jpeg(100));
    assert!(run(lib.path(), &["add", path.to_str().unwrap()]).status.success());

    let before = stdout_of(&run(lib.path(), &["list"]));
    assert!(before.contains("Only Book"));

    // `--really-do-it` reaches the command now: it used to be swallowed
    // as clap's program-name argument, so this printed the "you must
    // provide --really-do-it" notice and did nothing.
    let out = run(lib.path(), &["restore_database", "--really-do-it"]);
    let combined = format!("{}{}", stdout_of(&out), String::from_utf8_lossy(&out.stderr));
    assert!(
        !combined.contains("You must provide the --really-do-it option"),
        "the flag was not parsed: {combined}"
    );
    assert!(combined.contains("nothing to rebuild the database from"), "expected a refusal, got: {combined}");

    // The point of the guard: the library is still there.
    let after = stdout_of(&run(lib.path(), &["list"]));
    assert!(after.contains("Only Book"), "restore destroyed the index it could not rebuild: {after}");
}
