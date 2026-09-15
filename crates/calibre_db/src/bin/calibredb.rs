//! `calibredb`: the library-management CLI, port of
//! `old_src/src/calibre/db/cli/main.py`'s real entry point.
//!
//! Usage: `calibredb <command> --with-library <path> [command args...]`
//! (or `--library-path`, upstream's own second spelling of the same
//! flag). Dispatches into [`calibre_db::cli::main_dispatch::run_command`],
//! which already has a real implementation for every command upstream's
//! own `COMMANDS` tuple lists.
//!
//! # Disclosed narrowing
//!
//! Upstream's `--with-library` also accepts a `http://host:port/#library_id`
//! URL, connecting to a remote `calibre-server` instance instead of a
//! local library path (`dbctx.is_remote`). This port has no remote-library
//! client -- `--with-library` here is always a real local filesystem
//! path. `--username`/`--password`/`--timeout` (remote-connection-only
//! options) aren't ported for the same reason. No default library path
//! from calibre's own GUI settings either (this port has no persisted
//! GUI settings store yet) -- `--with-library` is required, not
//! optional, here.

use calibre_db::cli::main_dispatch::{run_command, DBCtx};

fn usage() -> ! {
    eprintln!("usage: calibredb <command> --with-library <path> [args...]");
    eprintln!();
    eprintln!("commands: list, add, remove, add_format, remove_format, show_metadata,");
    eprintln!("  set_metadata, export, catalog, saved_searches, add_custom_column,");
    eprintln!("  custom_columns, remove_custom_column, set_custom, restore_database,");
    eprintln!("  check_library, list_categories, backup_metadata, clone, embed_metadata,");
    eprintln!("  search, fts_index, fts_search, switch");
    std::process::exit(1);
}

/// Scans `args` for `--with-library`/`--library-path VALUE` (upstream's
/// own two spellings of the same global option) and pulls it out,
/// returning `(library_path, remaining_args)`. Real upstream's
/// `OptionParser` lets this global flag appear anywhere relative to
/// the command-specific ones; this matches that rather than requiring
/// a fixed position.
fn extract_library_path(args: Vec<String>) -> Option<(String, Vec<String>)> {
    let mut library_path = None;
    let mut remaining = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--with-library" || arg == "--library-path" {
            library_path = it.next();
        } else {
            remaining.push(arg);
        }
    }
    library_path.map(|p| (p, remaining))
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args.remove(0);

    let Some((library_path, cmd_args)) = extract_library_path(args) else {
        eprintln!("calibredb: --with-library is required");
        usage();
    };

    let ctx = DBCtx::new(std::path::PathBuf::from(library_path));
    if let Err(e) = run_command(&cmd, &cmd_args, &ctx) {
        eprintln!("calibredb: {e}");
        std::process::exit(1);
    }
}
