use calibre_db::cli::cmd_backup_metadata::CmdBackupMetadata;
use calibre_db::cli::cmd_catalog::CmdCatalog;
use calibre_db::cli::cmd_check_library::CmdCheckLibrary;
use std::path::Path;

#[test]
fn test_cli_maintenance_stubs() {
    use calibre_db::Library;
    let db = Library::open_test().unwrap();

    let backup = CmdBackupMetadata::new();
    let args = vec!["--all".to_string()];
    // Should now succeed
    assert!(backup.run(&db, &args).is_ok());

    let catalog = CmdCatalog::new();
    let p = Path::new("catalog.csv");
    // Catalog is likely still a stub, assuming signature hasn't changed
    assert!(catalog.run(p, "csv").is_err());

    let check = CmdCheckLibrary::new();
    let args = vec![];
    // Should now succeed
    assert!(check.run(&db, &args).is_ok());
}
