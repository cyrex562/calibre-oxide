use crate::restore;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Really do the recovery. The command will not run unless this option is specified.
    #[clap(long, short = 'r')]
    pub really_do_it: bool,

    /// Library path (defaults to current directory if not specified)
    #[clap(long, default_value = ".")]
    pub library_path: PathBuf,
}

pub struct CmdRestoreDatabase;

impl CmdRestoreDatabase {
    pub fn new() -> Self {
        CmdRestoreDatabase
    }

    pub fn run(&self, args: &[String]) -> anyhow::Result<()> {
        let run_args = RunArgs::parse_from(args);

        if !run_args.really_do_it {
            println!("You must provide the --really-do-it option to do a recovery");
            return Ok(());
        }

        let library_path = std::fs::canonicalize(run_args.library_path)?;
        println!("Restoring database at {:?}", library_path);

        restore::restore_database(library_path, |msg| {
            println!("{}", msg);
        })?;

        Ok(())
    }
}
