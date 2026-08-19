use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct RunArgs {
    // Add args as needed
}

pub struct CmdFitsIndex;

impl CmdFitsIndex {
    pub fn new() -> Self {
        CmdFitsIndex
    }

    pub fn run(&self, _args: &RunArgs) -> Result<()> {
        // Stub
        Err(anyhow::anyhow!("CmdFitsIndex::run is a stub"))
    }
}
