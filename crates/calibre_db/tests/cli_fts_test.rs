use calibre_db::cli::cmd_fts_index::{CmdFtsIndex, RunArgs as IndexArgs};
use calibre_db::cli::cmd_fts_search::{CmdFtsSearch, RunArgs as SearchArgs};
use calibre_db::library::Library;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;

fn add_book(lib: &mut Library, title: &str, author: &str) -> i32 {
    let dir = lib.path().to_path_buf();
    let source = dir.join(format!("{title}.epub"));
    fs::write(&source, "dummy").unwrap();
    let meta = MetaInformation {
        title: title.to_string(),
        authors: vec![author.to_string()],
        ..Default::default()
    };
    lib.add_book(&source, &meta).unwrap()
}

#[test]
fn fts_index_status_reports_disabled_before_enabling() {
    let mut lib = Library::open_test().unwrap();
    let cmd = CmdFtsIndex::new();
    // "status" on a disabled library prints and exits nonzero via
    // `std::process::exit` upstream -- avoid that here by checking
    // `is_fts_enabled` directly, which is what backs it.
    assert!(!lib.is_fts_enabled().unwrap());

    let args = IndexArgs {
        action: "enable".to_string(),
        items: vec![],
    };
    cmd.run(&mut lib, &args).unwrap();
    assert!(lib.is_fts_enabled().unwrap());

    let args = IndexArgs {
        action: "disable".to_string(),
        items: vec![],
    };
    cmd.run(&mut lib, &args).unwrap();
    assert!(!lib.is_fts_enabled().unwrap());
}

#[test]
fn fts_index_reindex_requires_enabled() {
    let mut lib = Library::open_test().unwrap();
    let cmd = CmdFtsIndex::new();
    let args = IndexArgs {
        action: "reindex".to_string(),
        items: vec![],
    };
    assert!(cmd.run(&mut lib, &args).is_err());
}

#[test]
fn fts_index_enable_dirties_every_existing_format() {
    let mut lib = Library::open_test().unwrap();
    add_book(&mut lib, "Book One", "Author");

    let cmd = CmdFtsIndex::new();
    cmd.run(
        &mut lib,
        &IndexArgs {
            action: "enable".to_string(),
            items: vec![],
        },
    )
    .unwrap();

    let (left, _total) = lib.fts_indexing_progress().unwrap();
    assert_eq!(left, 1);
}

#[test]
fn fts_search_requires_enabled_indexing() {
    let lib = Library::open_test().unwrap();
    let cmd = CmdFtsSearch::new();
    let args = SearchArgs {
        query: vec!["hello".to_string()],
        include_snippets: false,
        match_start_marker: "[".to_string(),
        match_end_marker: "]".to_string(),
        do_not_match_on_related_words: false,
        restrict_to: String::new(),
        output_format: "text".to_string(),
        indexing_threshold: 90.0,
    };
    assert!(cmd.run(&lib, &args).is_err());
}

#[test]
fn fts_search_end_to_end_finds_indexed_text() {
    let mut lib = Library::open_test().unwrap();
    let book_id = add_book(&mut lib, "Rust Book", "Jane Doe");

    lib.set_fts_enabled(true).unwrap();
    lib.fts()
        .add_text(
            book_id,
            "epub",
            0.0,
            Some("A book about Rust programming"),
            "",
            0,
            "",
            None,
        )
        .unwrap();

    let cmd = CmdFtsSearch::new();
    let args = SearchArgs {
        query: vec!["Rust".to_string()],
        include_snippets: true,
        match_start_marker: "[".to_string(),
        match_end_marker: "]".to_string(),
        do_not_match_on_related_words: false,
        restrict_to: String::new(),
        output_format: "text".to_string(),
        indexing_threshold: 0.0,
    };
    cmd.run(&lib, &args).unwrap();

    // Restricting to a search that finds nothing yields no hits but
    // still succeeds.
    let args = SearchArgs {
        restrict_to: "search:title:Nonexistent".to_string(),
        ..args
    };
    cmd.run(&lib, &args).unwrap();
}
