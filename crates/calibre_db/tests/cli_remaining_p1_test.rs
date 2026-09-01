use calibre_db::cli::cmd_embed_metadata::{CmdEmbedMetadata, RunArgs as EmbedArgs};
use calibre_db::cli::cmd_export::{CmdExport, RunArgs as ExportArgs};
use calibre_db::cli::cmd_fts_index::{CmdFtsIndex, RunArgs as FtsIndexArgs};
use calibre_db::cli::cmd_fts_search::{CmdFtsSearch, RunArgs as FtsSearchArgs};
use calibre_db::Library;

#[test]
fn test_cli_remaining_p1_stubs() {
    let db = Library::open_test().unwrap();

    let embed = CmdEmbedMetadata::new();
    // A nonexistent book id is a real error (nothing to embed into).
    assert!(embed.run(&db, &EmbedArgs { ids: vec!["999".to_string()] }).is_err());

    let export = CmdExport::new();
    let export_args = ExportArgs {
        ids: vec!["999".to_string()],
        all: false,
        to_dir: "export_dir".to_string(),
        single_dir: false,
        progress: false,
    };
    // A nonexistent book id simply exports nothing -- not an error.
    assert!(export.run(&db, &export_args).is_ok());

    let fts_idx = CmdFtsIndex::new();
    // "reindex" without FTS ever having been enabled is a real error
    // (see cli_fts_test.rs's own `fts_index_reindex_requires_enabled`).
    let mut db_mut = Library::open_test().unwrap();
    assert!(fts_idx.run(&mut db_mut, &FtsIndexArgs { action: "reindex".to_string(), items: vec![] }).is_err());

    let fts_search = CmdFtsSearch::new();
    let search_args = FtsSearchArgs {
        query: vec!["query".to_string()],
        include_snippets: false,
        match_start_marker: "[".to_string(),
        match_end_marker: "]".to_string(),
        do_not_match_on_related_words: false,
        restrict_to: String::new(),
        output_format: "text".to_string(),
        indexing_threshold: 90.0,
    };
    // Searching without FTS ever having been enabled is a real error.
    assert!(fts_search.run(&db, &search_args).is_err());
}
