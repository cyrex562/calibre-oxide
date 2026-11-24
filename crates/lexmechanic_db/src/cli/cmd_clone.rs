use crate::Library;
use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// Destination path
    #[arg(required = true)]
    pub path: PathBuf,
}

pub struct CmdClone;

impl CmdClone {
    pub fn new() -> Self {
        CmdClone
    }

    pub fn run(&self, db: &Library, args: &RunArgs) -> Result<()> {
        db.clone_to(&args.path).map_err(|e| anyhow::anyhow!(e))
    }
}
