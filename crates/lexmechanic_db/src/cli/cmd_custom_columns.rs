use crate::Library;
use serde_json::Value;
use std::collections::HashMap;

pub struct CmdCustomColumns;

impl CmdCustomColumns {
    pub fn new() -> Self {
        CmdCustomColumns
    }

    /// List available custom columns. Shows column labels and ids.
    /// Port of implementation in cmd_custom_columns.py
    pub fn run(&self, db: &Library, details: bool) -> anyhow::Result<()> {
        let custom_columns = db.get_custom_column_label_map()?;

        if details {
            for (col, data) in custom_columns {
                println!("{}", col);
                println!();
                // Roughly equivalent to pprint(pformat(data))
                // data is a HashMap<String, Value> or similar structure from JSON
                println!("{}", serde_json::to_string_pretty(&data)?);
                println!("\n");
            }
        } else {
            for (col, data) in custom_columns {
                // data['num']
                let num = data.get("num").and_then(|v| v.as_i64()).unwrap_or(0);
                println!("{} ({})", col, num);
            }
        }

        Ok(())
    }
}

// TODO: Add tests once we have a way to mock LibraryDatabase or a real temporary DB
