use crate::utils::integers_from_string;
use crate::Library;
use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// Comma separated list of ids (e.g. 1,2,3-10)
    #[arg(required = true, value_delimiter = ',', num_args = 1..)]
    pub ids: Vec<String>,

    /// Do not using the Recycle Bin
    #[arg(long)]
    pub permanent: bool,
}

pub struct CmdRemove;

impl CmdRemove {
    pub fn new() -> Self {
        CmdRemove
    }

    pub fn run(&self, db: &mut Library, args: &RunArgs) -> Result<()> {
        let mut ids = Vec::new();
        for id_str in &args.ids {
            ids.extend(integers_from_string(id_str));
        }

        if ids.is_empty() {
            return Err(anyhow::anyhow!(
                "You must specify at least one book to remove"
            ));
        }

        db.remove_books(&ids, args.permanent)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
