use crate::Library;
use anyhow::{Context, Result};
use std::io::{self, Write};

pub struct CmdSearch;

impl CmdSearch {
    pub fn new() -> Self {
        CmdSearch
    }

    pub fn run(&self, db: &Library, args: &[String]) -> Result<()> {
        let mut limit = -1;
        let mut query_parts = Vec::new();

        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--limit" | "-l" => {
                    idx += 1;
                    if idx < args.len() {
                        limit = args[idx].parse().context("Invalid limit value")?;
                    }
                }
                arg => query_parts.push(arg),
            }
            idx += 1;
        }

        let query = query_parts.join(" ");
        if query.trim().is_empty() {
            anyhow::bail!("Error: You must specify the search expression");
        }

        let ids = db.search(&query)?;

        // Apply limit if specified
        let ids = if limit > -1 {
            ids.into_iter().take(limit as usize).collect::<Vec<_>>()
        } else {
            ids
        };

        if ids.is_empty() {
            // In python it exits with message, here we can print and return
            println!("No books matching the search expression: {}", query);
            return Ok(());
        }

        // Output comma separated IDs
        let id_strings: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        print!("{}", id_strings.join(","));
        io::stdout().flush()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn test_cmd_search() {
        let mut db = Library::open_test().unwrap();
        db.insert_test_book("Test Book 1").unwrap();

        let cmd = CmdSearch::new();

        let args = vec!["Book".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());

        let args = vec!["--limit".to_string(), "1".to_string(), "Book".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());
    }
}
