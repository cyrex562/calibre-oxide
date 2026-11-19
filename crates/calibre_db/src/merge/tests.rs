use super::*;
use calibre_ebooks::metadata::MetaInformation;

struct Lib {
    dir: tempfile::TempDir,
    stage: tempfile::TempDir,
    cache: Cache,
}

fn lib() -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    Lib { dir, stage: tempfile::tempdir().unwrap(), cache }
}

impl Lib {
    /// A book with one format, copied in from outside the library.
    fn book(&self, stem: &str, ext: &str, bytes: &str, title: &str, author: &str) -> i32 {
        let source = self.stage.path().join(format!("{stem}.{ext}"));
        std::fs::write(&source, bytes).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        self.cache.add_book(&source, &meta).unwrap()
    }

    /// A book registered in place in a subfolder of the library.
    fn in_folder(&self, folder: &str, name: &str, bytes: &str) -> i32 {
        std::fs::create_dir_all(self.dir.path().join(folder)).unwrap();
        std::fs::write(self.dir.path().join(folder).join(name), bytes).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = name.to_string();
        self.cache.register_book_in_place(&format!("{folder}/{name}"), &meta).unwrap()
    }

    fn add_format(&self, id: i32, stem: &str, ext: &str, bytes: &str) {
        let source = self.stage.path().join(format!("{stem}.{ext}"));
        std::fs::write(&source, bytes).unwrap();
        assert!(self.cache.add_format(id, &source, ext, false).unwrap());
    }

    fn formats(&self, id: i32) -> Vec<String> {
        let mut v: Vec<String> = self.cache.format_file_info(id).unwrap().into_iter().map(|f| f.fmt.to_uppercase()).collect();
        v.sort();
        v
    }

    fn format_bytes(&self, id: i32, fmt: &str) -> String {
        let f = self.cache.format_file_info(id).unwrap().into_iter().find(|f| f.fmt.eq_ignore_ascii_case(fmt)).unwrap();
        std::fs::read_to_string(f.path).unwrap()
    }

    fn exists(&self, id: i32) -> bool {
        self.cache.field_for(id, "path").unwrap().is_some()
    }

    fn field(&self, id: i32, f: &str) -> String {
        self.cache.field_for(id, f).unwrap().unwrap_or_default()
    }
}

fn opts(mode: MergeMode) -> MergeOptions {
    MergeOptions { mode, ..Default::default() }
}

#[test]
fn formats_move_into_the_destination_and_the_source_goes() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    assert_eq!(l.formats(dest), vec!["EPUB", "PDF"]);
    assert_eq!(l.format_bytes(dest, "epub"), "src epub");
    assert!(!l.exists(src));
    assert_eq!(report.sources[0].formats_moved, vec!["EPUB"]);
    assert!(report.sources[0].deleted && report.sources[0].error.is_none());
    // The hash record followed the file.
    let rec = l.cache.checksums().find_by_hash(&blake3::hash(b"src epub").to_hex()).unwrap();
    assert!(rec.iter().any(|(b, k, f)| *b == dest && k == "format" && f == "EPUB"), "{rec:?}");
    assert!(!rec.iter().any(|(b, ..)| *b == src));
}

#[test]
fn a_conflicting_format_keeps_the_destinations_like_calibre() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf, small", "T", "A");
    let src = l.book("s", "pdf", "src pdf, much larger and newer", "T", "A");
    let src_path = l.cache.format_file_info(src).unwrap()[0].path.clone();
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    assert_eq!(l.format_bytes(dest, "pdf"), "dest pdf, small");
    assert_eq!(report.sources[0].formats_discarded, vec!["PDF"]);
    assert!(!l.exists(src));
    assert!(!src_path.exists(), "the discarded copy goes with the source (to the wastebasket)");
}

#[test]
fn keep_duplicate_formats_saves_the_other_copy_as_a_data_file() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "pdf", "src pdf", "T", "A");
    let o = MergeOptions { keep_duplicate_formats: true, ..Default::default() };
    let report = merge_books(&l.cache, dest, &[src], &o).unwrap();

    assert_eq!(l.format_bytes(dest, "pdf"), "dest pdf");
    let kept = crate::extra_files::list_extra_files(&l.cache, dest, "data/**/*").unwrap();
    assert_eq!(kept.len(), 1, "{report:?}");
    assert_eq!(std::fs::read_to_string(&kept[0].file_path).unwrap(), "src pdf");
    assert_eq!(report.sources[0].formats_kept_as_data_file[0].1, kept[0].relpath);
    assert!(!l.exists(src));
}

#[test]
fn a_byte_identical_duplicate_is_never_kept() {
    let l = lib();
    let dest = l.book("d", "pdf", "same bytes", "T", "A");
    let src = l.book("s", "pdf", "same bytes", "T", "A");
    let o = MergeOptions { keep_duplicate_formats: true, ..Default::default() };
    let report = merge_books(&l.cache, dest, &[src], &o).unwrap();
    assert_eq!(report.sources[0].formats_identical, vec!["PDF"]);
    assert!(crate::extra_files::list_extra_files(&l.cache, dest, "data/**/*").unwrap().is_empty());
    assert!(!l.exists(src));
    assert_eq!(l.formats(dest), vec!["PDF"]);
}

#[test]
fn keep_sources_mode_copies_and_merges_but_deletes_nothing() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    l.add_format(src, "s2", "pdf", "src other pdf");
    l.cache.set_field(src, "tags", "from-src").unwrap();
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeKeepSources)).unwrap();

    assert_eq!(l.formats(dest), vec!["EPUB", "PDF"]);
    assert_eq!(l.formats(src), vec!["EPUB", "PDF"], "the source keeps all of its own");
    assert!(l.cache.format_file_info(src).unwrap().iter().all(|f| f.path.is_file()));
    assert_eq!(l.field(dest, "tags"), "from-src");
    assert!(!report.sources[0].deleted);
    assert_eq!(report.sources[0].formats_left_on_source, vec!["PDF"]);
}

#[test]
fn formats_only_mode_leaves_the_destinations_metadata_alone() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    l.cache.set_field(src, "tags", "from-src").unwrap();
    l.cache.set_field(src, "publisher", "Pub").unwrap();
    merge_books(&l.cache, dest, &[src], &opts(MergeMode::FormatsOnly)).unwrap();
    assert_eq!(l.formats(dest), vec!["EPUB", "PDF"]);
    assert_eq!(l.field(dest, "tags"), "");
    assert_eq!(l.field(dest, "publisher"), "");
    assert!(!l.exists(src));
}

#[test]
fn metadata_follows_calibres_rules() {
    let l = lib();
    let dest = l.book("d", "pdf", "d", "Unknown", "Unknown");
    let s1 = l.book("s1", "epub", "s1", "Real Title", "Ada Lovelace");
    let s2 = l.book("s2", "mobi", "s2", "Other Title", "Someone Else");
    let c = &l.cache;
    c.set_field(dest, "comments", "dest comment").unwrap();
    c.set_field(dest, "tags", "a, b").unwrap();
    c.set_field(dest, "identifiers", "isbn:9").unwrap();
    c.set_field(dest, "rating", "4").unwrap();
    c.set_field(s1, "comments", "one").unwrap();
    c.set_field(s1, "tags", "B, c").unwrap();
    c.set_field(s1, "identifiers", "isbn:1,asin:2").unwrap();
    c.set_field(s1, "publisher", "Pub One").unwrap();
    c.set_field(s1, "rating", "9").unwrap();
    c.set_field(s1, "series", "Saga").unwrap();
    c.set_field(s1, "series_index", "3").unwrap();
    c.set_field(s1, "languages", "eng").unwrap();
    c.set_field(s2, "comments", "two").unwrap();
    c.set_field(s2, "tags", "d").unwrap();
    c.set_field(s2, "identifiers", "asin:99,goodreads:5").unwrap();
    c.set_field(s2, "publisher", "Pub Two").unwrap();
    c.set_field(s2, "series", "Other Saga").unwrap();
    c.set_field(s2, "languages", "eng, fra").unwrap();
    merge_books(c, dest, &[s1, s2], &opts(MergeMode::MergeAndDelete)).unwrap();

    assert_eq!(l.field(dest, "comments"), "dest comment\n\none\n\ntwo", "appended");
    assert_eq!(l.field(dest, "tags"), "a, b, c, d", "union, case-insensitive");
    assert_eq!(l.field(dest, "title"), "Real Title", "an Unknown title is filled, from the first source with one");
    assert_eq!(l.field(dest, "authors"), "Ada Lovelace");
    assert_eq!(l.field(dest, "publisher"), "Pub One", "first source with one fills an empty field");
    assert_eq!(l.field(dest, "rating"), "4", "calibre fills an empty rating, it does not take the max");
    assert_eq!(l.field(dest, "series"), "Saga");
    assert_eq!(l.field(dest, "series_index"), "3");
    assert_eq!(l.field(dest, "languages"), "eng, fra");
    let ids = l.field(dest, "identifiers");
    for want in ["isbn:9", "asin:2", "goodreads:5"] {
        assert!(ids.contains(want), "{ids}");
    }
    assert!(!ids.contains("isbn:1") && !ids.contains("asin:99"), "destination, then earlier source, win a clash: {ids}");
}

#[test]
fn a_rating_is_taken_only_when_the_destination_has_none() {
    let l = lib();
    let dest = l.book("d", "pdf", "d", "T", "A");
    let src = l.book("s", "epub", "s", "T", "A");
    l.cache.set_field(src, "rating", "10").unwrap();
    merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeKeepSources)).unwrap();
    assert_eq!(l.field(dest, "rating"), "10");
}

#[test]
fn a_merge_can_be_repeated_without_repeating_the_comments() {
    let l = lib();
    let dest = l.book("d", "pdf", "d", "T", "A");
    let src = l.book("s", "epub", "s", "T", "A");
    l.cache.set_field(dest, "comments", "mine").unwrap();
    l.cache.set_field(src, "comments", "theirs").unwrap();
    for _ in 0..2 {
        merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeKeepSources)).unwrap();
    }
    assert_eq!(l.field(dest, "comments"), "mine\n\ntheirs");
}

#[test]
fn the_cover_is_the_destinations_else_the_first_sources() {
    let l = lib();
    let (a, b, c) = (l.book("a", "pdf", "a", "T", "A"), l.book("b", "pdf", "b", "T", "A"), l.book("c", "pdf", "c", "T", "A"));
    crate::covers::set_cover(&l.cache, b, b"cover of b").unwrap();
    crate::covers::set_cover(&l.cache, c, b"cover of c").unwrap();
    let report = merge_books(&l.cache, a, &[b, c], &opts(MergeMode::MergeKeepSources)).unwrap();
    assert_eq!(report.cover_from, Some(b));
    assert_eq!(std::fs::read(crate::covers::cover_path(&l.cache, a).unwrap()).unwrap(), b"cover of b");

    // A destination that has one keeps it.
    let (d, e) = (l.book("d", "pdf", "dd", "T", "A"), l.book("e", "pdf", "ee", "T", "A"));
    crate::covers::set_cover(&l.cache, d, b"mine").unwrap();
    crate::covers::set_cover(&l.cache, e, b"theirs").unwrap();
    let report = merge_books(&l.cache, d, &[e], &opts(MergeMode::MergeKeepSources)).unwrap();
    assert_eq!(report.cover_from, None);
    assert_eq!(std::fs::read(crate::covers::cover_path(&l.cache, d).unwrap()).unwrap(), b"mine");
}

#[test]
fn books_sharing_a_real_folder_keep_their_siblings() {
    let l = lib();
    let root = l.dir.path().join("shelf");
    std::fs::create_dir_all(&root).unwrap();
    let mut ids = Vec::new();
    for (name, body, title) in [("one.pdf", "pdf bytes", "One"), ("two.epub", "epub bytes", "Two"), ("three.txt", "txt bytes", "Three")] {
        std::fs::write(root.join(name), body).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        ids.push(l.cache.register_book_in_place(&format!("shelf/{name}"), &meta).unwrap());
    }
    let (dest, src, sibling) = (ids[0], ids[1], ids[2]);
    assert_eq!(l.field(dest, "path"), l.field(sibling, "path"));

    merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    for name in ["one.pdf", "two.epub", "three.txt"] {
        assert!(root.join(name).is_file(), "{name} must survive");
    }
    assert_eq!(l.formats(dest), vec!["EPUB", "PDF"]);
    assert_eq!(l.formats(sibling), vec!["TXT"]);
    assert!(!l.exists(src));
    assert_eq!(l.format_bytes(dest, "epub"), "epub bytes");
}

#[test]
fn a_failure_after_the_copies_leaves_the_source_and_a_rerun_finishes() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    l.add_format(src, "s", "txt", "src txt");
    let src_files: Vec<_> = l.cache.format_file_info(src).unwrap().into_iter().map(|f| f.path).collect();

    FAIL_AT.with(|f| f.set(Some("after_formats")));
    let err = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete));
    FAIL_AT.with(|f| f.set(None));
    assert!(err.is_err());
    assert!(l.exists(src));
    assert!(src_files.iter().all(|p| p.is_file()), "nothing of the source was touched");
    assert_eq!(l.formats(dest), vec!["EPUB", "PDF", "TXT"], "the destination already has the lot");

    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();
    assert!(report.sources[0].deleted);
    assert!(!l.exists(src));
    assert_eq!(l.format_bytes(dest, "txt"), "src txt");
}

#[test]
fn a_failure_before_the_delete_leaves_a_complete_destination_and_the_source() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    FAIL_AT.with(|f| f.set(Some("before_delete")));
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();
    FAIL_AT.with(|f| f.set(None));
    assert!(report.sources[0].error.is_some() && !report.sources[0].deleted);
    assert!(l.exists(src));
    assert_eq!(l.formats(dest), vec!["EPUB", "PDF"]);
    assert!(l.cache.format_file_info(src).unwrap()[0].path.is_file());
}

#[test]
fn a_copy_that_fails_verification_is_undone_and_the_source_is_not_deleted() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.in_folder("shelf", "s.epub", "src epub");
    let src_path = l.cache.format_file_info(src).unwrap()[0].path.clone();
    CORRUPT_COPY.with(|c| c.set(true));
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();
    CORRUPT_COPY.with(|c| c.set(false));

    let s = &report.sources[0];
    assert!(s.error.as_deref().unwrap().contains("could not be verified"), "{s:?}");
    assert!(!s.deleted && l.exists(src));
    assert_eq!(std::fs::read_to_string(&src_path).unwrap(), "src epub");
    assert_eq!(l.formats(dest), vec!["PDF"], "the bad copy was removed");
}

#[test]
fn a_format_in_another_folder_is_copied_verified_and_the_original_trashed() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.in_folder("shelf", "s.epub", "src epub");
    let original = l.cache.format_file_info(src).unwrap()[0].path.clone();
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    let landed = l.cache.format_file_info(dest).unwrap().into_iter().find(|f| f.fmt.eq_ignore_ascii_case("epub")).unwrap().path;
    assert_ne!(landed, original);
    assert_eq!(std::fs::read_to_string(&landed).unwrap(), "src epub");
    assert!(!original.exists(), "{report:?}");
    assert_eq!(report.trashed(), vec!["shelf/s.epub"]);
    assert!(!l.exists(src));
}

#[test]
fn a_source_that_cannot_be_trashed_is_still_merged_and_its_files_stay() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "pdf", "other pdf", "T", "A");
    let path = l.cache.format_file_info(src).unwrap()[0].path.clone();
    crate::cache::FORCE_TRASH_FAILURE.with(|f| f.set(true));
    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();
    crate::cache::FORCE_TRASH_FAILURE.with(|f| f.set(false));
    assert_eq!(report.sources[0].failed.len(), 1);
    assert!(path.is_file(), "never hard-deleted");
}

fn data_files(c: &Cache, id: i32) -> Vec<String> {
    let mut v: Vec<String> = crate::extra_files::list_extra_files(c, id, "data/**/*").unwrap().into_iter().map(|f| f.relpath).collect();
    v.sort();
    v
}

/// The OS recycle bin does not exist on a network share, so a source's data
/// files must leave through the library's own trash: nothing stays in the
/// source's folder, nothing is lost, and a restore brings them back.
#[test]
fn a_sources_data_files_go_to_the_library_trash_and_come_back_on_restore() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.in_folder("shelf", "s.epub", "src epub");
    let mut files = std::collections::HashMap::new();
    files.insert("data/x/notes.txt".to_string(), b"first".to_vec());
    files.insert("data/y/notes.txt".to_string(), b"second".to_vec());
    files.insert("data/top.txt".to_string(), b"third".to_vec());
    crate::extra_files::add_extra_files(&l.cache, src, &files, true).unwrap();
    assert_eq!(data_files(&l.cache, src).len(), 3);

    let report = merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    assert!(report.sources[0].failed.is_empty(), "{report:?}");
    assert!(!l.exists(src));
    let landed: Vec<String> = crate::extra_files::list_extra_files(&l.cache, dest, "data/**/*").unwrap().into_iter().map(|f| std::fs::read_to_string(f.file_path).unwrap()).collect();
    assert_eq!(landed.len(), 3, "{landed:?}");
    for want in ["first", "second", "third"] {
        assert!(landed.iter().any(|c| c == want), "{want} missing from {landed:?}");
    }
    // Nothing left behind: the source's folder is gone entirely.
    assert!(!l.dir.path().join("shelf").exists(), "the source's folder was left behind");
    // In the library trash, and restorable.
    let entry = l.cache.list_trash().unwrap().into_iter().find(|e| e.book_id == src).expect("a trash entry");
    assert!(entry.problem.is_none(), "{entry:?}");
    l.cache.restore_trashed_book(&entry.key).unwrap();
    assert_eq!(std::fs::read_to_string(l.dir.path().join("shelf/data/x/notes.txt")).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(l.dir.path().join("shelf/data/y/notes.txt")).unwrap(), "second");
    assert_eq!(std::fs::read_to_string(l.dir.path().join("shelf/data/top.txt")).unwrap(), "third");
}

/// A root-level book keeps its data under `data/<uuid>/`.
#[test]
fn a_root_books_data_files_are_trashed_with_it_too() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    let mut files = std::collections::HashMap::new();
    files.insert("data/n.txt".to_string(), b"note".to_vec());
    crate::extra_files::add_extra_files(&l.cache, src, &files, true).unwrap();
    let uuid = l.cache.book_uuid(src).unwrap().unwrap();
    let original = l.dir.path().join("data").join(&uuid).join("n.txt");
    assert!(original.is_file());

    merge_books(&l.cache, dest, &[src], &opts(MergeMode::MergeAndDelete)).unwrap();

    assert!(!original.exists());
    assert!(!l.dir.path().join("data").join(&uuid).exists(), "the emptied data folder was left behind");
    assert_eq!(data_files(&l.cache, dest), vec!["data/n.txt"]);
    assert!(l.cache.list_trash().unwrap().iter().any(|e| e.book_id == src));
}

#[test]
fn a_dry_run_reports_and_changes_nothing() {
    let l = lib();
    let dest = l.book("d", "pdf", "dest pdf", "T", "A");
    let src = l.book("s", "epub", "src epub", "T", "A");
    l.add_format(src, "s2", "pdf", "src other pdf");
    l.cache.set_field(src, "tags", "x").unwrap();
    let o = MergeOptions { dry_run: true, ..Default::default() };
    let report = merge_books(&l.cache, dest, &[src], &o).unwrap();
    assert_eq!(report.sources[0].formats_moved, vec!["EPUB"]);
    assert!(report.sources[0].deleted, "predicted");
    // The epub shares the destination's folder, so it is claimed, not trashed;
    // only the pdf that loses to the destination's is.
    assert_eq!(report.sources[0].trashed.len(), 1, "{report:?}");
    assert!(report.sources[0].trashed[0].ends_with(".pdf"));
    assert_eq!(l.formats(dest), vec!["PDF"]);
    assert!(l.exists(src));
    assert_eq!(l.field(dest, "tags"), "");
}

#[test]
fn bad_input_is_refused_before_anything_is_touched() {
    let l = lib();
    let dest = l.book("d", "pdf", "d", "T", "A");
    assert!(merge_books(&l.cache, dest, &[], &MergeOptions::default()).is_err());
    assert!(merge_books(&l.cache, dest, &[dest], &MergeOptions::default()).is_err());
    assert!(merge_books(&l.cache, dest, &[9999], &MergeOptions::default()).is_err());
    assert!(merge_books(&l.cache, 9999, &[dest], &MergeOptions::default()).is_err());
    assert!(l.exists(dest));
}

#[test]
fn best_destination_prefers_formats_then_metadata_then_cover_then_age() {
    let s = |id, formats, metadata, has_cover| BookSummary { book_id: id, formats, metadata, has_cover };
    assert_eq!(best_destination(&[s(1, 1, 5, true), s(2, 2, 0, false)]), Some(2));
    assert_eq!(best_destination(&[s(1, 2, 1, true), s(2, 2, 3, false)]), Some(2));
    assert_eq!(best_destination(&[s(1, 2, 3, false), s(2, 2, 3, true)]), Some(2));
    assert_eq!(best_destination(&[s(7, 2, 3, true), s(3, 2, 3, true)]), Some(3));
    assert_eq!(best_destination(&[]), None);
}

fn content_duplicate_library() -> (Lib, Vec<(i32, i32)>) {
    let l = lib();
    let mut pairs = Vec::new();
    for n in 0..3 {
        let a = l.book(&format!("a{n}"), "pdf", &format!("same {n}"), &format!("Title {n}"), "A");
        let b = l.book(&format!("b{n}"), "pdf", &format!("same {n}"), &format!("Other title {n}"), "B");
        pairs.push((a, b));
    }
    // Same title and author, different content: must never be planned.
    l.book("m1", "pdf", "meta one", "Same Title", "Same Author");
    l.book("m2", "pdf", "meta two", "Same Title", "Same Author");
    (l, pairs)
}

#[test]
fn the_bulk_plan_covers_exact_content_groups_only_and_executes_what_it_planned() {
    let (l, pairs) = content_duplicate_library();
    let plan = plan_content_merges(&l.cache).unwrap();
    assert_eq!(plan.len(), 3, "{plan:?}");
    for (a, b) in &pairs {
        let g = plan.iter().find(|g| g.dest == *a || g.dest == *b).unwrap();
        assert_eq!(g.dest, *a, "equal records: the older one survives");
        assert_eq!(g.sources, vec![*b]);
    }
    let before = l.cache.all_book_ids().unwrap().len();
    let report = execute_content_merges(&l.cache, &plan, false, &crate::scan_pipeline::JobControl::new());
    assert_eq!((report.groups_merged, report.books_merged), (3, 3), "{report:?}");
    assert_eq!(l.cache.all_book_ids().unwrap().len(), before - 3);
    assert!(plan_content_merges(&l.cache).unwrap().is_empty());
}

#[test]
fn a_cancelled_bulk_run_stops_between_groups() {
    let (l, _) = content_duplicate_library();
    let plan = plan_content_merges(&l.cache).unwrap();
    let control = crate::scan_pipeline::JobControl::new();
    control.cancel();
    let report = execute_content_merges(&l.cache, &plan, false, &control);
    assert!(report.cancelled && report.groups_merged == 0);
    assert_eq!(plan_content_merges(&l.cache).unwrap().len(), 3);
}

#[test]
fn overlapping_content_groups_are_joined_into_one() {
    let l = lib();
    let a = l.book("a", "pdf", "shared pdf", "T", "A");
    let b = l.book("b", "pdf", "shared pdf", "T", "A");
    l.add_format(b, "b", "epub", "shared epub");
    let c = l.book("c", "epub", "shared epub", "T", "A");
    let plan = plan_content_merges(&l.cache).unwrap();
    assert_eq!(plan.len(), 1, "{plan:?}");
    assert_eq!(plan[0].dest, b, "b has the most formats");
    let mut all: Vec<i32> = plan[0].sources.clone();
    all.push(plan[0].dest);
    all.sort_unstable();
    assert_eq!(all, vec![a, b, c]);
}
