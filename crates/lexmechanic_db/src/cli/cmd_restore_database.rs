use crate::restore;
use clap::Parser;

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Really do the recovery. The command will not run unless this option is specified.
    #[clap(long, short = 'r')]
    pub really_do_it: bool,

}

/// `calibredb restore_database`.
///
/// Legacy, and deliberately so (#951): it rebuilds `metadata.db` from
/// `<author>/<title>/metadata.opf` files, which only a library in the
/// pre-#889 layout has. Recovery for a library written by this
/// application is a replay of the change log (#899) -- see
/// [`crate::restore`]'s module documentation for why there is one
/// recovery mechanism rather than two.
pub struct CmdRestoreDatabase;

impl CmdRestoreDatabase {
    pub fn new() -> Self {
        CmdRestoreDatabase
    }

    /// `library_path` comes from the shared `DBCtx`, like every other
    /// command's does.
    ///
    /// `RunArgs` used to declare its own `--library-path` defaulting to
    /// `"."`, which could never be set: `bin/calibredb.rs`'s
    /// `extract_library_path` strips both `--with-library` and
    /// `--library-path` out of the argument list before dispatch. So the
    /// value was always the default, and a recovery would have run
    /// against the current working directory instead of the library the
    /// user named.
    pub fn run(&self, library_path: &std::path::Path, args: &[String]) -> anyhow::Result<()> {
        // `parse_from` treats its first element as the program name and
        // discards it, so passing the bare argument list silently ate
        // `--really-do-it` and the command could never be invoked at
        // all. Every other clap-based arm in `main_dispatch` prepends
        // the command name for exactly this reason.
        let cmd_name = "restore_database".to_string();
        let run_args = RunArgs::parse_from(std::iter::once(&cmd_name).chain(args.iter()));

        if !run_args.really_do_it {
            println!("You must provide the --really-do-it option to do a recovery");
            return Ok(());
        }

        let library_path = std::fs::canonicalize(library_path)?;
        println!("Restoring database at {:?}", library_path);

        restore::restore_database(library_path, |msg| {
            println!("{}", msg);
        })?;

        Ok(())
    }
}
