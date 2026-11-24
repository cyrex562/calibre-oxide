//! Regression tests for the data-integrity and performance fixes of tranche
//! 22b: each one pins a finding from the code review.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::cache::{BookFiles, Cache};
use crate::organize::{self, Scope};
use crate::scan::{walk, ScanOptions};
use crate::scan_pipeline::JobControl;

fn library() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn index(dir: &Path, cache: &Cache) {
    let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
    crate::scan::index_scan(cache, dir, &report);
}

fn book_with_title(cache: &Cache, title: &str) -> i32 {
    cache.all_book_ids().unwrap().into_iter().find(|id| cache.field_for(*id, "title").unwrap().unwrap_or_default() == title).unwrap_or_else(|| panic!("no book titled {title}"))
}

/// The names in a directory, with their real spelling.
fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

// --- 1: the field cache is patched, not reloaded -----------------------

#[test]
fn organizing_fifty_books_reloads_the_field_cache_a_constant_number_of_times() {
    let (dir, cache) = library();
    for i in 0..50 {
        write(&dir.path().join(format!("inbox/Book {i:02}.pdf")), format!("%PDF book {i}").as_bytes());
    }
    index(dir.path(), &cache);
    assert_eq!(cache.all_book_ids().unwrap().len(), 50);

    let before = cache.field_cache_full_loads();
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
    assert_eq!(plan.books.len(), 50);
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
    assert_eq!(report.books_moved, 50, "{report:?}");
    // Every book is written (path, names) and then read back (covers, the next
    // book's fields). That used to be a full reload of every table per book.
    let loads = cache.field_cache_full_loads() - before;
    assert!(loads <= 2, "50 books caused {loads} full reloads of the field cache");
    // And the patched cache is right.
    for i in 0..50 {
        let id = book_with_title(&cache, &format!("Book {i:02}"));
        assert_eq!(cache.field_for(id, "path").unwrap().as_deref(), Some("Sorted"));
    }
}

// --- 2 and 8: case-only renames ----------------------------------------

#[test]
fn a_case_only_file_rename_is_not_refused() {
    let (dir, cache) = library();
    write(&dir.path().join("the hobbit.pdf"), b"%PDF hobbit");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "the hobbit");

    let renamed = cache.rename_format_files(id, "The Hobbit").unwrap();

    assert_eq!(renamed, vec!["PDF".to_string()]);
    assert_eq!(names_in(dir.path()).iter().filter(|n| n.ends_with(".pdf")).cloned().collect::<Vec<_>>(), vec!["The Hobbit.pdf".to_string()]);
    assert_eq!(cache.format_file_names(id).unwrap(), vec![("PDF".to_string(), "The Hobbit".to_string())]);
}

#[test]
fn a_rename_onto_a_different_existing_file_is_still_refused() {
    let (dir, cache) = library();
    write(&dir.path().join("one.pdf"), b"%PDF one");
    write(&dir.path().join("Other.pdf"), b"%PDF other");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "one");
    let err = cache.rename_format_files(id, "Other").unwrap_err();
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    assert_eq!(std::fs::read(dir.path().join("Other.pdf")).unwrap(), b"%PDF other");
}

#[test]
fn the_network_tier_accepts_a_case_only_rename() {
    let (dir, cache) = library();
    // Installed before any real handle is opened (see the note in cache.rs's
    // tests about the lock file).
    cache.backend.install_network_tier_handle_for_test();
    write(&dir.path().join("a.txt"), b"data");
    let handle = cache.backend.write_handle().unwrap();
    handle.rename_atomic(&dir.path().join("a.txt"), &dir.path().join("A.txt")).unwrap();
    assert!(names_in(dir.path()).contains(&"A.txt".to_string()));
    assert!(!names_in(dir.path()).contains(&"a.txt".to_string()));
}

// --- 7: never overwrite, even in the race window ------------------------

#[test]
fn a_no_replace_rename_refuses_an_existing_target_and_leaves_both_files() {
    let (dir, cache) = library();
    let handle = cache.backend.write_handle().unwrap();
    write(&dir.path().join("from.pdf"), b"mine");
    write(&dir.path().join("to.pdf"), b"theirs");

    let err = handle.rename_atomic_no_replace(&dir.path().join("from.pdf"), &dir.path().join("to.pdf")).unwrap_err();

    assert!(format!("{err}").to_lowercase().contains("exist"), "{err}");
    assert_eq!(std::fs::read(dir.path().join("to.pdf")).unwrap(), b"theirs");
    assert_eq!(std::fs::read(dir.path().join("from.pdf")).unwrap(), b"mine");

    handle.rename_atomic_no_replace(&dir.path().join("from.pdf"), &dir.path().join("free.pdf")).unwrap();
    assert_eq!(std::fs::read(dir.path().join("free.pdf")).unwrap(), b"mine");
    assert!(!dir.path().join("from.pdf").exists());
}

#[test]
fn organize_does_not_replace_a_file_that_appeared_after_the_plan() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
    // Somebody puts a file at the target between the plan and the run.
    write(&dir.path().join("Sorted/Alpha.pdf"), b"%PDF theirs");

    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();

    assert_eq!(report.books_moved, 0, "{report:?}");
    assert_eq!(report.failed.len(), 1);
    assert_eq!(std::fs::read(dir.path().join("Sorted/Alpha.pdf")).unwrap(), b"%PDF theirs");
    assert!(dir.path().join("inbox/Alpha.pdf").is_file());
}

// --- 3: the record is one transaction -----------------------------------

#[test]
fn a_failed_location_write_leaves_the_path_and_every_name_as_they_were() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Book.pdf"), b"%PDF a");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "Book");
    cache.record_format_row(id, "EPUB", "Book", 3, None).unwrap();
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute_batch("CREATE TEMP TRIGGER refuse_boom BEFORE UPDATE ON data WHEN NEW.name = 'boom' BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    }
    let old_path = cache.field_for(id, "path").unwrap();

    // Formats are written in order; EPUB (the first) succeeds and PDF fails.
    let names: BTreeMap<String, String> = [("EPUB".to_string(), "renamed".to_string()), ("PDF".to_string(), "boom".to_string())].into();
    let err = cache.set_book_location(id, Some("Elsewhere"), &names).unwrap_err();

    assert!(format!("{err:#}").contains("refused"), "{err:#}");
    assert_eq!(cache.field_for(id, "path").unwrap(), old_path, "the path did not move");
    assert_eq!(cache.format_file_names(id).unwrap(), vec![("EPUB".to_string(), "Book".to_string()), ("PDF".to_string(), "Book".to_string())], "no name was changed");
}

#[test]
fn when_the_library_cannot_record_a_move_the_files_go_back_and_the_reason_says_so() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
    index(dir.path(), &cache);
    let alpha = book_with_title(&cache, "Alpha");
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute_batch(&format!("CREATE TEMP TRIGGER refuse_alpha BEFORE UPDATE OF path ON books WHEN OLD.id = {alpha} BEGIN SELECT RAISE(ABORT, 'refused'); END;")).unwrap();
    }
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();

    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();

    assert_eq!(report.books_moved, 1, "{report:?}");
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("could not record"), "{:?}", report.failed);
    assert!(dir.path().join("inbox/Alpha.pdf").is_file(), "the file went back");
    assert!(!dir.path().join("Sorted/Alpha.pdf").exists());
    // Only the book that moved is in the run.
    let run = organize::load_run(dir.path(), &report.run_id).unwrap();
    assert_eq!(run.books.len(), 1);
    assert!(run.pending.is_empty());
}

// --- 4: the undo record exists before the move --------------------------

#[test]
fn a_crash_between_the_entry_and_the_record_still_leaves_something_undo_can_use() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
    let book = &plan.books[0];

    // What execute does for a book, stopped after the files move: the entry is
    // on disk, the move is done, and the database was never told.
    let run_id = "org-1-crash";
    let header = organize::OrganizeRun { run_id: run_id.into(), template: plan.template.clone(), created_at: 1, ..Default::default() };
    let mut journal = crate::run_journal::ORGANIZE_RUNS.begin(dir.path(), run_id, &header).unwrap();
    journal.append_book(book).unwrap();
    std::fs::create_dir_all(dir.path().join("Sorted")).unwrap();
    std::fs::rename(dir.path().join(&book.files[0].from), dir.path().join(&book.files[0].to)).unwrap();
    drop(journal); // the process dies here

    let run = organize::load_run(dir.path(), run_id).unwrap();
    assert_eq!(run.books.len(), 1, "the move is in the undo record");
    assert_eq!(run.pending, vec![book.book_id]);

    let report = organize::undo(&cache, run_id, &JobControl::new()).unwrap();
    assert_eq!(report.books_restored, 1, "{report:?}");
    assert!(dir.path().join("inbox/Alpha.pdf").is_file(), "the file is back where the database says it is");
    assert!(!dir.path().join("Sorted/Alpha.pdf").exists());
}

#[test]
fn a_run_lists_and_undoes_from_its_journal() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
    assert_eq!(report.books_moved, 2);
    assert!(dir.path().join(".lexmechanic/organize-runs").join(format!("{}.jsonl", report.run_id)).is_file());
    assert_eq!(organize::list_runs(dir.path()), vec![(report.run_id.clone(), organize::load_run(dir.path(), &report.run_id).unwrap().created_at, 2, false)]);

    let undo = organize::undo(&cache, &report.run_id, &JobControl::new()).unwrap();

    assert_eq!(undo.books_restored, 2, "{undo:?}");
    assert!(undo.warnings.is_empty());
    assert!(organize::list_runs(dir.path())[0].3, "marked undone");
    assert!(dir.path().join("inbox/Alpha.pdf").is_file());
}

#[test]
fn failing_to_mark_a_run_undone_is_a_warning_not_an_error() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
    let journal = dir.path().join(".lexmechanic/organize-runs").join(format!("{}.jsonl", report.run_id));
    let mut perms = std::fs::metadata(&journal).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&journal, perms.clone()).unwrap();

    let undo = organize::undo(&cache, &report.run_id, &JobControl::new());

    // Whatever the platform let the write do, the undo itself succeeded.
    let undo = undo.expect("a failed bookkeeping write must not fail the undo");
    assert_eq!(undo.books_restored, 1);
    assert!(dir.path().join("inbox/Alpha.pdf").is_file());
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&journal, perms).unwrap();
}

#[test]
fn retitle_records_its_entry_before_it_changes_anything() {
    use crate::retitle::{self, ApplyOptions, Item};
    let (dir, cache) = library();
    write(&dir.path().join("inbox/scan0001.pdf"), b"%PDF a");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "scan0001");
    let items = vec![Item { book_id: id, expected_title: "scan0001".into(), title: "A Real Title".into() }];
    let report = retitle::apply(&cache, &items, &ApplyOptions { embed: false, rename_files: false, rename_template: String::new() }, &JobControl::new()).unwrap();

    assert_eq!(report.retitled, 1, "{report:?}");
    assert!(dir.path().join(".lexmechanic/retitle-runs").join(format!("{}.jsonl", report.run_id)).is_file());
    let run = retitle::load_run(dir.path(), &report.run_id).unwrap();
    assert_eq!(run.books.len(), 1);
    assert_eq!(run.books[0].old_title, "scan0001");
    assert_eq!(run.books[0].new_title, "A Real Title");

    let undo = retitle::undo(&cache, &report.run_id, &JobControl::new()).unwrap();
    assert_eq!(undo.titles_restored, 1);
    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("scan0001"));
}

// --- 5: a change that missed the log is not forgotten -------------------

#[test]
fn a_failed_log_append_is_remembered_and_blocks_a_rebuild() {
    let (dir, cache) = library();
    let id = cache.insert_book_with_uuid("00000000-0000-0000-0000-000000000001").unwrap();
    cache.set_field(id, "title", "First").unwrap();
    assert!(cache.change_log_gap().is_none());

    // Make the log's directory unusable.
    let changes = dir.path().join(".lexmechanic").join("changes");
    assert!(changes.is_dir(), "the first write created the log directory");
    std::fs::remove_dir_all(&changes).unwrap();
    std::fs::write(&changes, b"not a directory").unwrap();

    cache.set_field(id, "title", "Second").unwrap();

    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Second"), "the write itself succeeded");
    let gap = cache.change_log_gap().expect("the missed append was recorded");
    assert_eq!(gap.missed, 1);
    let err = cache.rebuild_from_change_log().unwrap_err();
    assert!(format!("{err:#}").contains("behind"), "{err:#}");

    cache.clear_change_log_gap().unwrap();
    assert!(cache.change_log_gap().is_none());
}

// --- 6: trash expiry keeps a resolve run restorable ---------------------

#[test]
fn trash_entries_of_a_resolve_run_outlive_plain_deletes() {
    let (dir, cache) = library();
    write(&dir.path().join("a/One.pdf"), b"%PDF one");
    write(&dir.path().join("b/Two.pdf"), b"%PDF two");
    index(dir.path(), &cache);
    let one = book_with_title(&cache, "One");
    let two = book_with_title(&cache, "Two");
    crate::trash::with_trash_run("run-keep-1", || cache.delete_book_with(one, BookFiles::Trash)).unwrap();
    cache.delete_book_with(two, BookFiles::Trash).unwrap();
    assert_eq!(cache.list_trash().unwrap().len(), 2);

    let fourteen_days = 14 * 86400;
    let now = crate::trash::now_secs();
    // 30 days on: the plain delete has expired, the run's entry has not.
    let report = cache.expire_old_trash_at(now + 30 * 86400, fourteen_days).unwrap();
    assert_eq!(report.removed.len(), 1, "{report:?}");
    let left = cache.list_trash().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].run_id.as_deref(), Some("run-keep-1"));
    assert!(left[0].expires_at - left[0].deleted_at >= 180 * 86400, "the listing shows the longer expiry");
    // The run can still be restored.
    let restored = cache.restore_trash_run("run-keep-1").unwrap();
    assert_eq!(restored.restored.len(), 1, "{restored:?}");

    // And it does expire eventually.
    crate::trash::with_trash_run("run-keep-2", || cache.delete_book_with(one, BookFiles::Trash)).unwrap();
    let report = cache.expire_old_trash_at(now + 200 * 86400, fourteen_days).unwrap();
    assert_eq!(report.removed.len(), 1, "{report:?}");
}

// --- 9: rendered template components -------------------------------------

#[test]
fn a_reserved_device_name_in_a_title_is_mapped_aside() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/con.pdf"), b"%PDF con");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "{title}", &Scope::All).unwrap();
    assert_eq!(plan.books.len(), 1, "{plan:?}");
    assert_eq!(plan.books[0].files[0].to, "_con.pdf");
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
    assert_eq!(report.books_moved, 1, "{report:?}");
    assert!(dir.path().join("_con.pdf").is_file());
}

#[test]
fn a_very_long_destination_is_warned_about() {
    let (dir, cache) = library();
    let long = "x".repeat(110);
    write(&dir.path().join(format!("inbox/{long}.pdf")), b"%PDF long");
    index(dir.path(), &cache);
    let plan = organize::plan(&cache, "{title}/{title}/{title}", &Scope::All).unwrap();
    assert_eq!(plan.books.len(), 1, "it is still planned");
    assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
    assert!(plan.warnings[0].reason.contains("characters long"));
}

#[test]
fn a_template_that_renders_into_a_link_out_of_the_library_is_refused() {
    let (dir, cache) = library();
    let outside = tempfile::tempdir().unwrap();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    let link = dir.path().join("Alpha");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(outside.path(), &link).is_ok();
    if !made {
        eprintln!("skipped: this account may not create symlinks");
        return;
    }

    let plan = organize::plan(&cache, "{title}/{title}", &Scope::All).unwrap();

    assert!(plan.books.is_empty(), "{plan:?}");
    assert_eq!(plan.skipped.len(), 1);
    assert!(plan.skipped[0].reason.contains("link out of the library"), "{:?}", plan.skipped);
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing was written outside");
}

// --- 10: one case-folding rule --------------------------------------------

#[test]
fn a_folder_is_found_whatever_case_it_is_asked_for_in() {
    let (dir, cache) = library();
    write(&dir.path().join("École/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    // Typed in lower case, as a user might: the same folder.
    let plan = crate::library_folders::plan_rename(&cache, "école", "Nouveau").unwrap();
    assert_eq!(plan.books.len(), 1, "{plan:?}");
    assert_eq!(plan.books[0].new_path, "Nouveau");
}

#[test]
fn a_case_only_folder_rename_renames_the_directory_on_disk() {
    let (dir, cache) = library();
    write(&dir.path().join("Fiction/Alpha.pdf"), b"%PDF alpha");
    write(&dir.path().join("Fiction/Beta.pdf"), b"%PDF beta");
    index(dir.path(), &cache);

    let plan = crate::library_folders::plan_rename(&cache, "Fiction", "fiction").unwrap();
    assert_eq!(plan.case_fix_folder.as_deref(), Some("fiction"));
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();

    assert_eq!(report.books_moved, 2, "{report:?}");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let names = names_in(dir.path());
    assert!(names.contains(&"fiction".to_string()), "{names:?}");
    assert!(!names.contains(&"Fiction".to_string()), "{names:?}");
    let alpha = book_with_title(&cache, "Alpha");
    assert_eq!(cache.field_for(alpha, "path").unwrap().as_deref(), Some("fiction"));
    assert!(dir.path().join("fiction/Alpha.pdf").is_file());
}

/// The undo of a case-only rename puts the directory's old spelling back too,
/// not just the books' records (tranche 24a).
#[test]
fn undoing_a_case_only_folder_rename_restores_the_directory_spelling() {
    let (dir, cache) = library();
    write(&dir.path().join("Fiction/Alpha.pdf"), b"%PDF alpha");
    write(&dir.path().join("Fiction/Beta.pdf"), b"%PDF beta");
    index(dir.path(), &cache);

    let plan = crate::library_folders::plan_rename(&cache, "Fiction", "fiction").unwrap();
    let report = organize::execute(&cache, &plan, &JobControl::new()).unwrap();
    assert_eq!(report.books_moved, 2, "{report:?}");
    assert!(names_in(dir.path()).contains(&"fiction".to_string()));

    // The rename is in the run's journal, written before it was made.
    let run = organize::load_run(dir.path(), &report.run_id).unwrap();
    let on_a_case_insensitive_volume = !run.dir_case.is_empty();
    if on_a_case_insensitive_volume {
        assert_eq!(run.dir_case, vec![organize::DirCaseRename { parent: String::new(), from: "Fiction".into(), to: "fiction".into() }]);
    }

    let undone = organize::undo(&cache, &report.run_id, &JobControl::new()).unwrap();
    assert_eq!(undone.books_restored, 2, "{undone:?}");
    assert!(undone.warnings.is_empty(), "{:?}", undone.warnings);
    let names = names_in(dir.path());
    assert!(names.contains(&"Fiction".to_string()), "the old spelling is back: {names:?}");
    assert!(!names.contains(&"fiction".to_string()), "{names:?}");
    assert!(dir.path().join("Fiction/Alpha.pdf").is_file());
    let alpha = book_with_title(&cache, "Alpha");
    assert_eq!(cache.field_for(alpha, "path").unwrap().as_deref(), Some("Fiction"));
}

// --- 11: delete is one transaction ----------------------------------------

#[test]
fn a_delete_that_fails_part_way_keeps_the_books_checksums() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "Alpha");
    cache.checksums().record_hash(id, "format", "PDF", "abc123", 10).unwrap();
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute_batch("CREATE TEMP TRIGGER refuse_delete BEFORE DELETE ON books BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    }

    let err = cache.delete_book_with(id, BookFiles::Keep).unwrap_err();

    assert!(format!("{err:#}").contains("refused"), "{err:#}");
    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Alpha"), "the book is still there");
    let (hash, _) = cache.checksums().recorded_identity(id, "format", "PDF").unwrap();
    assert!(hash.is_some(), "and it still has its hash: the checksums were not deleted first");
}

// --- 13: relocating a file streams it -------------------------------------

#[test]
fn relocating_a_large_file_records_its_real_hash_and_size() {
    let (dir, cache) = library();
    write(&dir.path().join("inbox/Big.pdf"), b"%PDF seed");
    index(dir.path(), &cache);
    let id = book_with_title(&cache, "Big");
    // Bigger than the streaming chunk, and not a multiple of it.
    let big: Vec<u8> = (0..(2 * 1024 * 1024 + 12345)).map(|i| (i % 251) as u8).collect();
    write(&dir.path().join("moved/Big.pdf"), &big);

    crate::orphans::relocate(&cache, id, "PDF", &dir.path().join("moved/Big.pdf")).unwrap();

    let (hash, _) = cache.checksums().recorded_identity(id, "format", "PDF").unwrap();
    assert_eq!(hash.unwrap(), blake3::hash(&big).to_hex().to_string());
    assert_eq!(cache.field_for(id, "size").unwrap().unwrap(), big.len().to_string());
}
