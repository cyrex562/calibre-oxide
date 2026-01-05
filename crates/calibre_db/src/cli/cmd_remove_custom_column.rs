use crate::Library;
use anyhow::{Context, Result};

pub struct CmdRemoveCustomColumn;

impl CmdRemoveCustomColumn {
    pub fn new() -> Self {
        CmdRemoveCustomColumn
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        let label = args.get(0).context("Missing argument: label")?;

        db.remove_custom_column(label)
            .context("Failed to remove custom column")?;

        println!("Removed custom column '{}'", label);
        Ok(())
    }
}
