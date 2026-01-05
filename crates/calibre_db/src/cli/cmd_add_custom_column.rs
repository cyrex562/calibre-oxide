use crate::Library;
use anyhow::{Context, Result};

pub struct CmdAddCustomColumn;

impl CmdAddCustomColumn {
    pub fn new() -> Self {
        CmdAddCustomColumn
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        let mut is_multiple = false;
        let mut positional_args = Vec::new();

        for arg in args {
            if arg == "--is-multiple" {
                is_multiple = true;
            } else {
                positional_args.push(arg);
            }
        }

        if positional_args.len() < 3 {
            anyhow::bail!("Usage: add_custom_column [--is-multiple] <label> <name> <datatype>");
        }

        let label = positional_args[0].as_str();
        let name = positional_args[1].as_str();
        let datatype = positional_args[2].as_str();

        let col_id = db
            .add_custom_column(label, name, datatype, is_multiple)
            .context("Failed to add custom column")?;

        println!("Added custom column '{}' with ID {}", label, col_id);
        Ok(())
    }
}
