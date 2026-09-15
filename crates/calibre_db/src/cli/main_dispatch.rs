use crate::Library;
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;

// Existing commands modules
use super::{
    cmd_add,
    cmd_add_custom_column,
    cmd_add_format,
    // Add others if needed after implementation
    cmd_backup_metadata,
    cmd_catalog,
    cmd_check_library,
    cmd_clone,
    cmd_custom_columns,
    cmd_embed_metadata,
    cmd_export,
    cmd_fts_index,
    cmd_fts_search,
    cmd_list,
    cmd_list_categories,
    cmd_remove,
    cmd_remove_custom_column,
    cmd_remove_format,
    cmd_restore_database,
    cmd_saved_searches,
    cmd_search,
    cmd_set_custom,
    cmd_set_metadata,
    cmd_show_metadata,
    cmd_switch,
};

pub struct DBCtx {
    pub library_path: PathBuf,
    // Add more fields as needed, e.g. remote connection info
}

impl DBCtx {
    pub fn new(library_path: PathBuf) -> Self {
        Self { library_path }
    }

    pub fn db(&self) -> Result<Library> {
        Library::open(self.library_path.clone()).map_err(|e| anyhow!(e))
    }
}

pub fn run_command(cmd: &str, args: &[String], ctx: &DBCtx) -> Result<()> {
    match cmd {
        "custom_columns" => {
            let details =
                args.contains(&"--details".to_string()) || args.contains(&"-d".to_string());
            let db = ctx.db()?;
            cmd_custom_columns::CmdCustomColumns::new().run(&db, details)
        }
        "list" => {
            let db = ctx.db()?;
            cmd_list::CmdList::new().run(&db, args)
        }
        "search" => {
            let db = ctx.db()?;
            cmd_search::CmdSearch::new().run(&db, args)
        }
        "show_metadata" => {
            let db = ctx.db()?;
            cmd_show_metadata::CmdShowMetadata::new().run(&db, args)
        }
        "add" => {
            let mut db = ctx.db()?;
            cmd_add::CmdAdd::new().run(&mut db, args)
        }
        "add_custom_column" => {
            let mut db = ctx.db()?;
            cmd_add_custom_column::CmdAddCustomColumn::new().run(&mut db, args)
        }
        "add_format" => {
            let mut db = ctx.db()?;
            cmd_add_format::CmdAddFormat::new().run(&mut db, args)
        }
        "backup_metadata" => {
            let db = ctx.db()?;
            cmd_backup_metadata::CmdBackupMetadata::new().run(&db, args)
        }
        "check_library" => {
            let db = ctx.db()?;
            cmd_check_library::CmdCheckLibrary::new().run(&db, args)
        }
        "catalog" => {
            let db = ctx.db()?;
            let cmd_name = "catalog".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_catalog::RunArgs::parse_from(clap_args);
            cmd_catalog::CmdCatalog::new().run(&db, &run_args)
        }
        "export" => {
            let db = ctx.db()?;
            let cmd_name = "export".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_export::RunArgs::parse_from(clap_args);
            cmd_export::CmdExport::new().run(&db, &run_args)
        }
        "embed_metadata" => {
            let db = ctx.db()?;
            let cmd_name = "embed_metadata".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_embed_metadata::RunArgs::parse_from(clap_args);
            cmd_embed_metadata::CmdEmbedMetadata::new().run(&db, &run_args)
        }
        "fts_index" => {
            let mut db = ctx.db()?;
            let cmd_name = "fts_index".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_fts_index::RunArgs::parse_from(clap_args);
            cmd_fts_index::CmdFtsIndex::new().run(&mut db, &run_args)
        }
        "fts_search" => {
            let db = ctx.db()?;
            let cmd_name = "fts_search".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_fts_search::RunArgs::parse_from(clap_args);
            cmd_fts_search::CmdFtsSearch::new().run(&db, &run_args)
        }
        "clone" => {
            let db = ctx.db()?;
            let cmd_name = "clone".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_clone::RunArgs::parse_from(clap_args);
            cmd_clone::CmdClone::new().run(&db, &run_args)
        }
        "list_categories" => {
            let db = ctx.db()?;
            cmd_list_categories::CmdListCategories::new().run(&db, args)
        }
        "remove" => {
            let mut db = ctx.db()?;
            let cmd_name = "remove".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_remove::RunArgs::parse_from(clap_args);
            cmd_remove::CmdRemove::new().run(&mut db, &run_args)
        }
        "remove_custom_column" => {
            let mut db = ctx.db()?;
            cmd_remove_custom_column::CmdRemoveCustomColumn::new().run(&mut db, args)
        }
        "saved_searches" => {
            let mut db = ctx.db()?;
            cmd_saved_searches::CmdSavedSearches::new().run(&mut db, args)
        }
        "set_custom" => {
            let mut db = ctx.db()?;
            cmd_set_custom::CmdSetCustom::new().run(&mut db, args)
        }
        "set_metadata" => {
            let mut db = ctx.db()?;
            cmd_set_metadata::CmdSetMetadata::new().run(&mut db, args)
        }
        "remove_format" => {
            let mut db = ctx.db()?;
            cmd_remove_format::CmdRemoveFormat::new().run(&mut db, args)
        }
        "switch" => cmd_switch::CmdSwitch::new().run(args),
        "restore_database" => cmd_restore_database::CmdRestoreDatabase::new().run(args),

        _ => Err(anyhow!("Unknown command: {}", cmd)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    /// `run_command("add", ...)` is the real dispatch path the
    /// `calibredb` binary uses -- unlike `cmd_add`'s own unit test,
    /// which calls `CmdAdd::run` directly and would have kept passing
    /// even while this dispatcher's `"add"` arm was a hardcoded
    /// `Err(...)` stub. Exercises the dispatcher itself, on a real
    /// on-disk library (`DBCtx::db()` requires one via
    /// `Library::open`, unlike `Library::open_test()`).
    #[test]
    fn run_command_add_really_adds_a_book_through_the_dispatcher() {
        let lib_dir = tempfile::tempdir().unwrap();
        Library::create(lib_dir.path().to_path_buf()).unwrap();
        let ctx = DBCtx::new(lib_dir.path().to_path_buf());

        let book_dir = tempfile::tempdir().unwrap();
        let book_path = book_dir.path().join("Dispatch Test.epub");
        let mut f = fs::File::create(&book_path).unwrap();
        f.write_all(b"dummy content").unwrap();

        run_command("add", &[book_path.to_string_lossy().to_string()], &ctx).unwrap();

        let db = ctx.db().unwrap();
        let books = db.list_books().unwrap();
        assert_eq!(books.len(), 1, "the dispatcher should have really added the book, not stubbed out");
    }

    #[test]
    fn run_command_reports_an_unknown_command_as_an_error() {
        let lib_dir = tempfile::tempdir().unwrap();
        Library::create(lib_dir.path().to_path_buf()).unwrap();
        let ctx = DBCtx::new(lib_dir.path().to_path_buf());
        assert!(run_command("not_a_real_command", &[], &ctx).is_err());
    }
}
