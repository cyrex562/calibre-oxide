use crate::Library;
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;

// Existing commands modules
use super::{
    cmd_add_custom_column,
    cmd_add_format,
    // Add others if needed after implementation
    cmd_backup_metadata,
    // cmd_fits_index,
    cmd_catalog,
    cmd_check_library,
    cmd_clone,
    cmd_custom_columns,
    cmd_embed_metadata,
    cmd_export,
    cmd_fits_index,
    cmd_fits_search,
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
        // Stub all others to avoid import/signature issues during porting
        "add" => Err(anyhow!(
            "Command '{}' not yet implemented in main_dispatch",
            cmd
        )),
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
        "fits_index" => {
            let cmd_name = "fits_index".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_fits_index::RunArgs::parse_from(clap_args);
            cmd_fits_index::CmdFitsIndex::new().run(&run_args)
        }
        "fits_search" => {
            let cmd_name = "fits_search".to_string();
            let clap_args = std::iter::once(&cmd_name).chain(args.iter());
            let run_args = cmd_fits_search::RunArgs::parse_from(clap_args);
            cmd_fits_search::CmdFitsSearch::new().run(&run_args)
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
