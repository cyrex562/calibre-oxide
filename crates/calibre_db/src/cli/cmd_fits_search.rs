use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// Search query
    #[arg(required = true)]
    pub query: String,
}

pub struct CmdFitsSearch;

impl CmdFitsSearch {
    pub fn new() -> Self {
        CmdFitsSearch
    }

    pub fn run(&self, _args: &RunArgs) -> Result<()> {
        // Stub
        Err(anyhow::anyhow!("CmdFitsSearch::run is a stub"))
    }
}
