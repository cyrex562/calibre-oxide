use crate::Library;
use anyhow::{anyhow, Result};

pub struct CmdSetCustom;

impl CmdSetCustom {
    pub fn new() -> Self {
        CmdSetCustom
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        if args.len() < 3 {
            return Err(anyhow!("Usage: set_custom <book_id> <label> <value>"));
        }

        let book_id = args[0]
            .parse::<i32>()
            .map_err(|_| anyhow!("Invalid book_id"))?;
        let label = &args[1];
        let value = args[2..].join(" "); // value might have spaces

        db.set_custom_column_value(book_id, label, &value)?;
        println!(
            "Set custom column '{}' for book {} to '{}'",
            label, book_id, value
        );
        Ok(())
    }
}
