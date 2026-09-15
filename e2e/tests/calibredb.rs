//! Real, black-box tests of the compiled `calibredb` binary (#704) --
//! run through an actual subprocess against an actual on-disk
//! library, not `calibre_db::cli::main_dispatch::run_command` called
//! in-process. This is the layer that would have caught `"add"` being
//! a hardcoded stub: `cmd_add.rs`'s own unit test called `CmdAdd::run`
//! directly and stayed green the whole time.

use std::process::Command;

use calibre_oxide_e2e::{oxide_bin, upstream_calibredb, TestLibrary};

fn run_calibredb(library: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(oxide_bin("calibredb")).args(args).arg("--with-library").arg(library).output().expect("failed to run the compiled calibredb binary")
}

#[test]
fn add_then_list_round_trips_a_real_book_through_the_real_binary() {
    let lib = TestLibrary::new();
    // A `.epub` extension whose bytes aren't a real zip makes
    // `get_metadata` really fail -- see cmd_add.rs::add_single_file's
    // fallback-to-filename-as-title arm, which this test wants to
    // exercise (unlike `.txt`, whose own format-specific metadata
    // reader tolerates unparseable content by defaulting to "Unknown"
    // instead of erroring -- a different, also-real code path).
    let book = lib.path().join("A Real Book.epub");
    std::fs::write(&book, "not really an epub").unwrap();

    let add = run_calibredb(lib.path(), &["add", book.to_str().unwrap()]);
    assert!(add.status.success(), "calibredb add failed: {}", String::from_utf8_lossy(&add.stderr));
    assert!(String::from_utf8_lossy(&add.stdout).contains("Added book id"), "got: {}", String::from_utf8_lossy(&add.stdout));

    let list = run_calibredb(lib.path(), &["list"]);
    assert!(list.status.success());
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.contains("A Real Book"), "expected the added book in `list` output, got: {stdout}");
}

#[test]
fn reports_an_unknown_command_as_a_real_process_failure() {
    let lib = TestLibrary::new();
    let out = run_calibredb(lib.path(), &["not_a_real_command"]);
    assert!(!out.status.success(), "an unknown command should be a real non-zero exit, not a silent success");
}

#[test]
fn fails_clearly_against_a_library_path_that_does_not_exist() {
    let out = Command::new(oxide_bin("calibredb")).args(["list", "--with-library"]).arg("/no/such/path/at/all").output().unwrap();
    assert!(!out.status.success());
}

/// Live comparison against a real, installed upstream `calibredb`
/// (e.g. `apt install calibre`) -- skipped, not failed, on a box
/// without it. Confirms real *behavioral* parity (a book added from a
/// file this port's/upstream's own metadata sniffer can't parse still
/// gets added, falling back to the filename as title and "Unknown" as
/// author) rather than exact CLI output formatting, which this port's
/// `calibredb list` deliberately doesn't attempt to match byte-for-
/// byte (see crates/calibre_db/src/bin/calibredb.rs's own module doc).
#[test]
fn upstream_calibredb_shows_the_same_filename_fallback_behavior() {
    let Some(upstream) = upstream_calibredb() else {
        eprintln!("skipping: no real upstream calibredb installed on this box");
        return;
    };

    let upstream_lib = tempfile::tempdir().unwrap();
    let book = upstream_lib.path().join("Fallback Title.epub");
    std::fs::write(&book, "not a real ebook").unwrap();

    let add = Command::new(&upstream).args(["add", book.to_str().unwrap(), "--with-library"]).arg(upstream_lib.path()).output().expect("failed to run the real upstream calibredb");
    assert!(add.status.success(), "upstream calibredb add failed: {}", String::from_utf8_lossy(&add.stderr));

    let list = Command::new(&upstream).args(["list", "--with-library"]).arg(upstream_lib.path()).output().unwrap();
    let upstream_out = String::from_utf8_lossy(&list.stdout);
    assert!(upstream_out.contains("Fallback Title"), "upstream should fall back to the filename as title, got: {upstream_out}");
    assert!(upstream_out.contains("Unknown"), "upstream should fall back to 'Unknown' as author, got: {upstream_out}");

    // This port's own binary, same scenario.
    let lib = TestLibrary::new();
    let port_book = lib.path().join("Fallback Title.epub");
    std::fs::write(&port_book, "not a real ebook").unwrap();
    let add = run_calibredb(lib.path(), &["add", port_book.to_str().unwrap()]);
    assert!(add.status.success());
    let list = run_calibredb(lib.path(), &["list"]);
    let port_out = String::from_utf8_lossy(&list.stdout);
    assert!(port_out.contains("Fallback Title"), "this port should fall back to the filename as title too, got: {port_out}");
    assert!(port_out.contains("Unknown"), "this port should fall back to 'Unknown' as author too, got: {port_out}");
}
