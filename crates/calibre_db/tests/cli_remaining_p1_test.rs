use calibre_db::cli::cmd_embed_metadata::CmdEmbedMetadata;
use calibre_db::cli::cmd_export::CmdExport;
use calibre_db::cli::cmd_fits_index::CmdFitsIndex;
use calibre_db::cli::cmd_fits_search::CmdFitsSearch;
use std::path::Path;

#[test]
fn test_cli_remaining_p1_stubs() {
    let embed = CmdEmbedMetadata::new();
    assert!(embed.run(&[1, 2, 3]).is_err());

    let export = CmdExport::new();
    assert!(export.run(Path::new("export_dir")).is_err());

    let fits_idx = CmdFitsIndex::new();
    assert!(fits_idx.run().is_err());

    let fits_search = CmdFitsSearch::new();
    assert!(fits_search.run("query").is_err());
}
