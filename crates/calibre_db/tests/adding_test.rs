use calibre_db::adding::{self, CompiledRule, FilterRuleConfig};
use calibre_db::cache::Cache;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_add_book() {
    let dir = tempdir().unwrap();

    // `Cache::new` opens (and, for a fresh dir, creates) the real
    // calibre schema via `Backend::new` -- no need to hand-roll one.
    let cache = Arc::new(Mutex::new(Cache::new(dir.path()).unwrap()));

    let authors = vec!["Author One".to_string(), "Author Two".to_string()];
    let book_id = adding::add_book(&cache, "New Book Title", &authors).expect("add_book failed");

    assert!(book_id > 0);

    // Verify Data
    let guard = cache.lock().unwrap();
    let title = guard.field_for(book_id, "title").unwrap().unwrap();
    let author_sort = guard.field_for(book_id, "author_sort").unwrap().unwrap();
    let uuid = guard.field_for(book_id, "uuid").unwrap();

    assert_eq!(title, "New Book Title");
    assert_eq!(author_sort, "Author One & Author Two");
    assert!(uuid.is_some());
    println!("Generated UUID: {}", uuid.unwrap());
}

fn touch(dir: &Path, name: &str) {
    fs::write(dir.join(name), "content").unwrap();
}

#[test]
fn find_books_in_directory_groups_one_book_per_directory() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "book.epub");
    touch(dir.path(), "book.mobi");
    touch(dir.path(), "cover.jpg"); // not a known book/metadata extension

    let groups = adding::find_books_in_directory(dir.path(), true, &[]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
}

#[test]
fn find_books_in_directory_groups_by_filename_stem_when_not_single_book() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "Foundation.epub");
    touch(dir.path(), "Foundation.mobi");
    touch(dir.path(), "Dune.epub");

    let groups = adding::find_books_in_directory(dir.path(), false, &[]);
    let mut sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![1, 2]);
}

#[test]
fn find_books_in_directory_ignores_unrecognized_extensions_without_rules() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "notes.xyz");
    let groups = adding::find_books_in_directory(dir.path(), true, &[]);
    assert!(groups.is_empty());
}

#[test]
fn compile_rule_glob_matches_case_insensitively() {
    let rule = FilterRuleConfig {
        match_type: "glob".to_string(),
        query: "*.tmp".to_string(),
        action: "add".to_string(),
    };
    let compiled = adding::compile_rule(&rule).unwrap();
    assert_eq!(adding::filter_filename(&[compiled], "FOO.TMP"), Some(true));
}

#[test]
fn compile_rule_not_prefix_negates_whether_the_rule_applies() {
    // "not_endswith" matches files that do *not* end with the query
    // -- the "not_" prefix negates whether the rule fires at all, not
    // its action. So a file that DOES end with ".part" makes this
    // rule not apply (falls through to `None`, no rule matched)...
    let rule = FilterRuleConfig {
        match_type: "not_endswith".to_string(),
        query: ".part".to_string(),
        action: "exclude".to_string(),
    };
    let compiled = adding::compile_rule(&rule).unwrap();
    assert_eq!(
        adding::filter_filename(std::slice::from_ref(&compiled), "book.part"),
        None
    );

    // ...while a file that does NOT end with ".part" makes it apply,
    // returning its (non-"add") action.
    assert_eq!(
        adding::filter_filename(&[adding::compile_rule(&rule).unwrap()], "book.epub"),
        Some(false)
    );
}

#[test]
fn filter_filename_returns_none_when_no_rule_matches() {
    let rule = FilterRuleConfig {
        match_type: "startswith".to_string(),
        query: "cover".to_string(),
        action: "add".to_string(),
    };
    let compiled = adding::compile_rule(&rule).unwrap();
    let rules: Vec<CompiledRule> = vec![compiled];
    assert_eq!(adding::filter_filename(&rules, "book.epub"), None);
}

#[test]
fn find_books_in_directory_uses_an_explicit_exclude_rule_over_the_extension_fallback() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "book.epub");
    touch(dir.path(), "draft.epub");

    let rule = FilterRuleConfig {
        match_type: "startswith".to_string(),
        query: "draft".to_string(),
        action: "exclude".to_string(),
    };
    let rules = vec![adding::compile_rule(&rule).unwrap()];

    let groups = adding::find_books_in_directory(dir.path(), true, &rules);
    // "draft.epub" is explicitly excluded, "book.epub" still passes
    // through the metadata-extension fallback since no rule matches it.
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 1);
    assert!(groups[0][0].ends_with("book.epub"));
}

#[test]
fn import_book_directory_adds_one_book_with_every_format_in_the_dir() {
    let src_dir = tempdir().unwrap();
    touch(src_dir.path(), "my_book.epub");
    touch(src_dir.path(), "my_book.mobi");

    let lib_dir = tempdir().unwrap();
    let cache = Arc::new(Mutex::new(Cache::new(lib_dir.path()).unwrap()));

    let book_id = adding::import_book_directory(&cache, src_dir.path(), &[])
        .unwrap()
        .expect("should have added a book");

    let guard = cache.lock().unwrap();
    let title = guard.field_for(book_id, "title").unwrap().unwrap();
    assert_eq!(title, "my book");
    // Directory read order isn't guaranteed, so check the format set
    // rather than a specific join order.
    let formats = guard.field_for(book_id, "formats").unwrap().unwrap();
    let mut formats: Vec<&str> = formats.split(", ").collect();
    formats.sort_unstable();
    assert_eq!(formats, vec!["EPUB", "MOBI"]);
}

#[test]
fn import_book_directory_multiple_adds_a_separate_book_per_stem() {
    let src_dir = tempdir().unwrap();
    touch(src_dir.path(), "Foundation.epub");
    touch(src_dir.path(), "Dune.epub");

    let lib_dir = tempdir().unwrap();
    let cache = Arc::new(Mutex::new(Cache::new(lib_dir.path()).unwrap()));

    let ids = adding::import_book_directory_multiple(&cache, src_dir.path(), &[]).unwrap();
    assert_eq!(ids.len(), 2);

    let guard = cache.lock().unwrap();
    let titles: std::collections::HashSet<String> = ids
        .iter()
        .map(|&id| guard.field_for(id, "title").unwrap().unwrap())
        .collect();
    assert!(titles.contains("Foundation"));
    assert!(titles.contains("Dune"));
}

#[test]
fn recursive_import_walks_subdirectories() {
    let src_dir = tempdir().unwrap();
    let sub = src_dir.path().join("subdir");
    fs::create_dir_all(&sub).unwrap();
    touch(&sub, "Nested Book.epub");

    let lib_dir = tempdir().unwrap();
    let cache = Arc::new(Mutex::new(Cache::new(lib_dir.path()).unwrap()));

    let ids = adding::recursive_import(&cache, src_dir.path(), true, &[]).unwrap();
    assert_eq!(ids.len(), 1);

    let guard = cache.lock().unwrap();
    assert_eq!(
        guard.field_for(ids[0], "title").unwrap().unwrap(),
        "Nested Book"
    );
}
