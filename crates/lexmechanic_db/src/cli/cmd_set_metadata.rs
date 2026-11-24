use crate::Library;
use anyhow::{anyhow, Result};

pub struct CmdSetMetadata;

impl CmdSetMetadata {
    pub fn new() -> Self {
        CmdSetMetadata
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        if args.len() < 3 {
            return Err(anyhow!("Usage: set_metadata <book_id> <field> <value>"));
        }

        let book_id = args[0]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid book_id"))?;
        let field = &args[1];
        let value = args[2..].join(" ");

        db.set_metadata(book_id, field, &value)?;
        println!(
            "Set metadata '{}' for book {} to '{}'",
            field, book_id, value
        );
        Ok(())
    }
}
