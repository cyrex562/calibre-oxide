use calibre_db::cli::cmd_backup_metadata::CmdBackupMetadata;
use calibre_db::cli::cmd_catalog::{CmdCatalog, RunArgs as CatalogRunArgs};
use calibre_db::cli::cmd_check_library::CmdCheckLibrary;

#[test]
fn test_cli_maintenance_stubs() {
    use calibre_db::Library;
    let db = Library::open_test().unwrap();

    let backup = CmdBackupMetadata::new();
    let args = vec!["--all".to_string()];
    // Should now succeed
    assert!(backup.run(&db, &args).is_ok());

    let temp_dir = tempfile::tempdir().unwrap();
    let catalog = CmdCatalog::new();
    let catalog_args = CatalogRunArgs {
        output_file: temp_dir.path().join("catalog.csv"),
        ids: None,
        search: None,
        verbose: false,
    };
    // A real, supported (CSV) catalog export against an empty library
    // succeeds -- it just writes the header row.
    assert!(catalog.run(&db, &catalog_args).is_ok());
    assert!(catalog_args.output_file.exists());

    let check = CmdCheckLibrary::new();
    let args = vec![];
    // Should now succeed
    assert!(check.run(&db, &args).is_ok());
}
