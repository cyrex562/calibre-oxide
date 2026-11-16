use super::*;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Arc;

fn library() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

/// A real, minimal EPUB whose OPF carries a title and an author, which is
/// what the importer must read instead of guessing from the filename.
fn epub(title: &str, author: &str, nonce: &str) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let stored = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", stored).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("OEBPS/content.opf", stored).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf"><dc:title>{title}</dc:title><dc:creator opf:role="aut">{author}</dc:creator><dc:identifier id="id">{nonce}</dc:identifier><dc:language>en</dc:language></metadata><manifest/><spine/></package>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    out.into_inner()
}

/// Every file under `root` with its bytes and modification time: what "the
/// source tree was not touched" has to compare.
fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, (fs::read(&path).unwrap(), fs::metadata(&path).unwrap().modified().unwrap()));
            }
        }
    }
    out
}

fn run(cache: &Cache, source: &Path, options: &ImportOptions) -> ImportReport {
    import_folder(cache, source, options, &PipelineOptions::with_workers(2), &JobControl::new()).unwrap()
}

fn titles(cache: &Cache) -> Vec<String> {
    let mut v: Vec<String> = cache.all_book_ids().unwrap().into_iter().map(|id| cache.field_for(id, "title").unwrap().unwrap()).collect();
    v.sort();
    v
}

#[test]
fn the_layout_is_calibres_and_the_metadata_is_the_files_own() {
    let (lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    // The filename says nothing useful; the OPF does.
    write(&src.path().join("scan0042.epub"), &epub("Dune", "Frank Herbert", "1"));
    let before = snapshot(src.path());

    let report = run(&cache, src.path(), &ImportOptions::default());

    assert_eq!(report.books_added, 1, "{report:?}");
    let id = report.added[0].book_id;
    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Dune"));
    assert_eq!(cache.field_for(id, "authors").unwrap().as_deref(), Some("Frank Herbert"));
    let folder = cache.field_for(id, "path").unwrap().unwrap();
    assert_eq!(folder, format!("Frank Herbert/Dune ({id})"));
    let file = lib.path().join(&folder).join("Dune - Frank Herbert.epub");
    assert!(file.is_file(), "{} missing", file.display());
    assert_eq!(fs::read(&file).unwrap(), before["scan0042.epub"].0);
    assert_eq!(cache.format_file_names(id).unwrap(), vec![("EPUB".to_string(), "Dune - Frank Herbert".to_string())]);
    // The hash on record is the verified copy's.
    let (hash, _) = cache.checksums().recorded_identity(id, "format", "EPUB").unwrap();
    assert_eq!(hash.as_deref(), Some(blake3::hash(&before["scan0042.epub"].0).to_hex().as_str()));
    // The staging area is empty again, and the source is exactly as it was.
    assert!(!lib.path().join(".calibre-oxide").join(STAGING_DIR_NAME).exists());
    assert_eq!(snapshot(src.path()), before, "the source tree was modified");
}

fn nested_tree(root: &Path) {
    write(&root.join("x.pdf"), b"%PDF x");
    write(&root.join("sub/a.pdf"), b"%PDF a");
    write(&root.join("sub/a.epub"), b"epub a not a real zip");
    write(&root.join("sub/b.pdf"), b"%PDF b");
    write(&root.join("sub/deep/c.pdf"), b"%PDF c");
    write(&root.join("sub/deep/notes.txt.bak"), b"not a book");
}

#[test]
fn multiple_books_per_folder_groups_formats_by_stem_through_nested_folders() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    nested_tree(src.path());
    let before = snapshot(src.path());

    let report = run(&cache, src.path(), &ImportOptions { mode: ImportMode::MultiplePerFolder, ..Default::default() });

    assert_eq!(report.files_seen, 5);
    assert_eq!(report.books_found, 4);
    assert_eq!(report.books_added, 4, "{report:?}");
    assert_eq!(titles(&cache), vec!["a", "b", "c", "x"]);
    let a = cache.all_book_ids().unwrap().into_iter().find(|id| cache.field_for(*id, "title").unwrap().as_deref() == Some("a")).unwrap();
    let mut formats: Vec<String> = cache.format_file_names(a).unwrap().into_iter().map(|(f, _)| f).collect();
    formats.sort();
    assert_eq!(formats, vec!["EPUB".to_string(), "PDF".to_string()], "a.pdf and a.epub are one book");
    assert!(report.conclusive);
    assert_eq!(snapshot(src.path()), before);
}

#[test]
fn one_book_per_folder_makes_each_folder_a_book_and_reports_the_second_file_of_a_format() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    nested_tree(src.path());

    let report = run(&cache, src.path(), &ImportOptions { mode: ImportMode::OneBookPerFolder, ..Default::default() });

    // root (x.pdf), sub (a.pdf + a.epub; b.pdf is a second PDF), deep (c.pdf).
    assert_eq!(report.books_found, 3);
    assert_eq!(report.books_added, 3, "{report:?}");
    assert_eq!(report.skipped_same_format.len(), 1, "{report:?}");
    assert!(report.skipped_same_format[0].ends_with("b.pdf"));
    assert_eq!(report.failed.len(), 0);
}

#[test]
fn not_recursive_stays_in_the_folder() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    nested_tree(src.path());

    let report = run(&cache, src.path(), &ImportOptions { recursive: false, ..Default::default() });

    assert_eq!(titles(&cache), vec!["x"], "{report:?}");
}

/// One file that cannot be read must not stop the others, and must be
/// named in the report.
#[test]
fn an_unreadable_file_is_collected_and_the_rest_still_import() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("good1.pdf"), b"%PDF 1");
    write(&src.path().join("locked.pdf"), b"%PDF locked");
    write(&src.path().join("good2.pdf"), b"%PDF 2");

    #[cfg(windows)]
    let _guard = {
        use std::os::windows::fs::OpenOptionsExt;
        // Share mode 0: nobody else may open it while this handle lives.
        std::fs::OpenOptions::new().read(true).share_mode(0).open(src.path().join("locked.pdf")).unwrap()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(src.path().join("locked.pdf"), fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(src.path().join("locked.pdf")).is_ok() {
            return; // running as root: cannot make a file unreadable
        }
    }

    let report = run(&cache, src.path(), &ImportOptions::default());

    assert_eq!(report.books_added, 2, "{report:?}");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].path.ends_with("locked.pdf"));
    assert_eq!(titles(&cache), vec!["good1", "good2"]);
}

/// A folder the walk cannot read is *reported*, and makes the report
/// inconclusive; it does not vanish.
#[test]
fn walk_errors_are_collected_not_dropped() {
    let mut errors = Vec::new();
    let control = JobControl::new();
    let missing = tempfile::tempdir().unwrap().path().join("gone");
    let extensions: HashSet<String> = ["pdf".to_string()].into_iter().collect();

    let found = walk(&missing, true, None, &extensions, &control, &mut errors);

    assert!(found.is_empty());
    assert_eq!(errors.len(), 1);
    assert!(errors[0].path.ends_with("gone"), "{errors:?}");

    // And through the job: an unreadable subfolder makes it inconclusive.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (_lib, cache) = library();
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("ok.pdf"), b"%PDF ok");
        let bad = src.path().join("bad");
        fs::create_dir(&bad).unwrap();
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&bad).is_err() {
            let report = run(&cache, src.path(), &ImportOptions::default());
            assert_eq!(report.books_added, 1);
            assert_eq!(report.walk_errors.len(), 1, "{report:?}");
            assert!(!report.conclusive);
        }
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn identical_content_is_skipped_by_default_and_only_the_default() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("one/copy.pdf"), b"%PDF the same bytes");
    write(&src.path().join("two/other name.pdf"), b"%PDF the same bytes");
    write(&src.path().join("three/different.pdf"), b"%PDF different");

    let report = run(&cache, src.path(), &ImportOptions::default());

    assert_eq!(report.books_added, 2, "{report:?}");
    assert_eq!(report.skipped_duplicate_content, 1);
    assert_eq!(report.duplicates.len(), 1);
    assert!(report.duplicates[0].path.ends_with("other name.pdf"), "{:?}", report.duplicates);

    // A second copy elsewhere, with new bytes around it, is still a duplicate
    // of what the library already has.
    let more = tempfile::tempdir().unwrap();
    write(&more.path().join("again.pdf"), b"%PDF different");
    let again = run(&cache, more.path(), &ImportOptions::default());
    assert_eq!(again.books_added, 0);
    assert_eq!(again.skipped_duplicate_content, 1);
    assert_eq!(again.duplicates[0].duplicate_of, Some(report.added.iter().find(|b| b.title == "different").unwrap().book_id));

    // "Add anyway" really does.
    let anyway = run(&cache, more.path(), &ImportOptions { policy: DuplicatePolicy::AddAnyway, ..Default::default() });
    assert_eq!(anyway.books_added, 1);
}

#[test]
fn title_and_author_matches_are_skipped_only_when_asked_and_only_with_a_real_author() {
    let (_lib, cache) = library();
    let first = tempfile::tempdir().unwrap();
    write(&first.path().join("dune.epub"), &epub("Dune", "Frank Herbert", "a"));
    run(&cache, first.path(), &ImportOptions::default());

    // Different bytes, same title and author.
    let second = tempfile::tempdir().unwrap();
    write(&second.path().join("dune (another scan).epub"), &epub("dune ", "frank herbert", "b"));
    write(&second.path().join("loose.pdf"), b"%PDF no metadata at all");

    // The default skips only identical content: this is a different file.
    let by_content = run(&cache, second.path(), &ImportOptions::default());
    assert_eq!(by_content.books_added, 2, "{by_content:?}");

    let (_lib2, cache2) = library();
    run(&cache2, first.path(), &ImportOptions::default());
    let by_title = run(&cache2, second.path(), &ImportOptions { policy: DuplicatePolicy::SkipTitleAuthor, ..Default::default() });
    assert_eq!(by_title.skipped_duplicate_title, 1, "{by_title:?}");
    // The PDF has no author to match on, so it is added, and counted.
    assert_eq!(by_title.books_added, 1);
    assert_eq!(by_title.unmatched_without_author, 1);
    assert_eq!(titles(&cache2), vec!["Dune", "loose"]);
}

#[test]
fn formats_are_added_to_the_existing_record_when_asked() {
    let (_lib, cache) = library();
    // The book is already here, as a PDF.
    let pdf_dir = tempfile::tempdir().unwrap();
    let pdf = pdf_dir.path().join("dune.pdf");
    write(&pdf, b"%PDF dune");
    let mut mi = calibre_ebooks::metadata::MetaInformation::default();
    mi.title = "Dune".to_string();
    mi.authors = vec!["Frank Herbert".to_string()];
    let id = cache.add_book(&pdf, &mi).unwrap();
    let second = tempfile::tempdir().unwrap();
    write(&second.path().join("dune.epub"), &epub("Dune", "Frank Herbert", "a"));

    let report = run(&cache, second.path(), &ImportOptions { policy: DuplicatePolicy::AddFormatsToExisting, ..Default::default() });

    assert_eq!(report.books_added, 0, "{report:?}");
    assert_eq!(report.formats_added_to_existing, 1, "{report:?}");
    let mut formats: Vec<String> = cache.format_file_names(id).unwrap().into_iter().map(|(f, _)| f).collect();
    formats.sort();
    assert_eq!(formats, vec!["EPUB".to_string(), "PDF".to_string()]);
    assert_eq!(cache.all_book_ids().unwrap().len(), 1);
    // A format the book already has is skipped, not replaced.
    let third = tempfile::tempdir().unwrap();
    write(&third.path().join("dune again.epub"), &epub("Dune", "Frank Herbert", "yet another"));
    let again = run(&cache, third.path(), &ImportOptions { policy: DuplicatePolicy::AddFormatsToExisting, ..Default::default() });
    assert_eq!(again.formats_added_to_existing, 0);
    assert_eq!(again.skipped_duplicate_title, 1, "{again:?}");
}

#[test]
fn a_rerun_skips_what_was_imported_without_reading_it() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    for i in 0..4 {
        write(&src.path().join(format!("f{i}.pdf")), format!("%PDF {i}").as_bytes());
    }
    let first = run(&cache, src.path(), &ImportOptions::default());
    assert_eq!(first.books_added, 4);

    // Even under "add anyway" a file that was imported is not imported again.
    let control = JobControl::new();
    let second = import_folder(&cache, src.path(), &ImportOptions { policy: DuplicatePolicy::AddAnyway, ..Default::default() }, &PipelineOptions::with_workers(1), &control).unwrap();

    assert_eq!(second.books_added, 0, "{second:?}");
    assert_eq!(second.skipped_already_imported, 4);
    assert_eq!(control.snapshot().file_opens, 0, "a file already imported must not be opened");
    assert_eq!(cache.all_book_ids().unwrap().len(), 4);

    // A file that changed is looked at again.
    write(&src.path().join("f0.pdf"), b"%PDF 0 but edited");
    let third = run(&cache, src.path(), &ImportOptions::default());
    assert_eq!(third.books_added, 1, "{third:?}");
}

#[test]
fn cancel_keeps_what_was_committed_and_a_rerun_finishes_the_rest() {
    let (_lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    for i in 0..6 {
        write(&src.path().join(format!("f{i}.pdf")), format!("%PDF {i}").as_bytes());
    }
    let before = snapshot(src.path());
    // One group per batch; cancel the moment the first batch has committed.
    let pipeline = PipelineOptions { workers: 1, batch_size: 4, after_batch: Some(Arc::new(|c: &JobControl| c.cancel())) };
    let control = JobControl::new();

    let first = import_folder(&cache, src.path(), &ImportOptions::default(), &pipeline, &control).unwrap();

    assert!(first.cancelled);
    assert!(!first.conclusive);
    assert_eq!(first.books_added, 1, "{first:?}");
    assert_eq!(cache.all_book_ids().unwrap().len(), 1);

    let second_control = JobControl::new();
    let second = import_folder(&cache, src.path(), &ImportOptions::default(), &PipelineOptions::with_workers(1), &second_control).unwrap();

    assert!(!second.cancelled);
    assert_eq!(second.books_added, 5, "{second:?}");
    assert_eq!(second.skipped_already_imported, 1);
    assert_eq!(second_control.snapshot().file_opens, 5, "only the five new files were read");
    assert_eq!(cache.all_book_ids().unwrap().len(), 6);
    assert_eq!(snapshot(src.path()), before);
}

#[test]
fn a_copy_that_does_not_verify_is_discarded_and_reported() {
    let (lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("a.pdf"), b"%PDF a");

    CORRUPT_STAGED.with(|c| c.set(true));
    let report = import_folder(&cache, src.path(), &ImportOptions::default(), &PipelineOptions::with_workers(1), &JobControl::new()).unwrap();
    CORRUPT_STAGED.with(|c| c.set(false));

    assert_eq!(report.books_added, 0);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].error.contains("did not match"), "{report:?}");
    assert!(cache.all_book_ids().unwrap().is_empty());
    let seen = crate::scan::walk(lib.path(), &crate::scan::ScanOptions::default(), std::time::SystemTime::now() + std::time::Duration::from_secs(3600)).unwrap();
    assert!(seen.files.is_empty(), "a bad copy reached the library: {:?}", seen.files);
    // Not recorded as done, so a rerun retries it.
    let again = run(&cache, src.path(), &ImportOptions::default());
    assert_eq!(again.books_added, 1);
}

#[test]
fn a_source_inside_the_library_is_refused_and_a_library_inside_the_source_is_not_imported_into_itself() {
    let (lib, cache) = library();
    write(&lib.path().join("inside/a.pdf"), b"%PDF a");
    let err = import_folder(&cache, &lib.path().join("inside"), &ImportOptions::default(), &PipelineOptions::with_workers(1), &JobControl::new()).unwrap_err();
    assert!(format!("{err:#}").contains("inside the library"), "{err:#}");

    // The library lives inside the folder being imported.
    let outer = tempfile::tempdir().unwrap();
    let nested = outer.path().join("my library");
    fs::create_dir(&nested).unwrap();
    let cache = Cache::new(&nested).unwrap();
    write(&nested.join("Someone/Old (1)/Old - Someone.pdf"), b"%PDF already in the library");
    write(&outer.path().join("loose.pdf"), b"%PDF loose");
    let report = run(&cache, outer.path(), &ImportOptions::default());
    assert_eq!(report.books_added, 1, "{report:?}");
    assert_eq!(titles(&cache), vec!["loose"]);
}

#[test]
fn an_import_survives_a_rebuild_from_the_change_log() {
    let (lib, cache) = library();
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("d.epub"), &epub("Dune", "Frank Herbert", "1"));
    write(&src.path().join("e.pdf"), b"%PDF e");
    let report = run(&cache, src.path(), &ImportOptions::default());
    assert_eq!(report.books_added, 2);

    let rebuilt_dir = tempfile::tempdir().unwrap();
    let state = LIBRARY_HANDLE_DIR_NAME;
    for sub in ["changes", "snapshots"] {
        let to = rebuilt_dir.path().join(state).join(sub);
        fs::create_dir_all(&to).unwrap();
        if let Ok(read) = fs::read_dir(lib.path().join(state).join(sub)) {
            for e in read.flatten() {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    fs::copy(lib.path().join(state).join("install-id"), rebuilt_dir.path().join(state).join("install-id")).ok();
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();

    for id in cache.all_book_ids().unwrap() {
        let uuid = cache.book_uuid(id).unwrap().unwrap();
        let rid = rebuilt.book_id_for_uuid(&uuid).unwrap().expect("book missing after rebuild");
        for field in ["title", "authors", "path"] {
            assert_eq!(rebuilt.field_for(rid, field).unwrap(), cache.field_for(id, field).unwrap(), "{field}");
        }
        assert_eq!(rebuilt.format_file_names(rid).unwrap(), cache.format_file_names(id).unwrap());
    }
}

#[test]
fn windows_verbatim_prefixes_are_not_shown_to_people() {
    assert_eq!(path_string(Path::new("\\\\?\\C:\\Books\\x.pdf")), "C:\\Books\\x.pdf");
    assert_eq!(path_string(Path::new("\\\\?\\UNC\\nas\\share\\x.pdf")), "\\\\nas\\share\\x.pdf");
    assert_eq!(path_string(Path::new("/home/me/x.pdf")), "/home/me/x.pdf");
}

#[test]
fn names_follow_calibres_construct_path_name() {
    let (folder, stem) = crate::cache::import_names(7, "A/B: C?", "Jane. ");
    assert_eq!(folder, "Jane/A_B_ C_ (7)");
    assert_eq!(stem, "A_B_ C_ - Jane");
    let long = "x".repeat(300);
    let (folder, stem) = crate::cache::import_names(12345, &long, &long);
    assert!(folder.ends_with(" (12345)"), "{folder}");
    assert!(folder.len() < 220 && stem.len() < 80, "{} {}", folder.len(), stem.len());
}
