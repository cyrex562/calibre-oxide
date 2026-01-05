use crate::Library;
use anyhow::{Context, Result};
use std::io::{self, Write};

pub struct CmdList;

impl CmdList {
    pub fn new() -> Self {
        CmdList
    }

    pub fn run(&self, db: &Library, args: &[String]) -> Result<()> {
        let mut limit = -1;
        let mut search_query = None;
        let mut for_machine = false;

        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--limit" | "-l" => {
                    idx += 1;
                    if idx < args.len() {
                        limit = args[idx].parse().context("Invalid limit value")?;
                    }
                }
                "--for-machine" => {
                    for_machine = true;
                }
                "-s" | "--search" => {
                    idx += 1;
                    if idx < args.len() {
                        search_query = Some(args[idx].clone());
                    }
                }
                arg => {
                    // Check if it's a flag we missed or positional.
                    // cmd_list in python uses option parser, so args usually require flags except maybe fields?
                    // For now, if we haven't consumed it, ignore or treat as error?
                    // Python: `calibre-db list [options]`. No positional args usually.
                }
            }
            idx += 1;
        }

        let books = if let Some(query) = search_query {
            let ids = db.search(&query)?;
            let mut matched_books = Vec::new();
            for id in ids {
                if let Some(book) = db.get_book(id)? {
                    matched_books.push(book);
                }
            }
            matched_books
        } else {
            db.list_books()?
        };

        let books = if limit > -1 {
            books.into_iter().take(limit as usize).collect::<Vec<_>>()
        } else {
            books
        };

        if for_machine {
            let json = serde_json::to_string_pretty(&books)?;
            println!("{}", json);
        } else {
            // Simple text table
            println!("{:<5} {:<40} {:<20}", "ID", "Title", "Author");
            println!("{}", "-".repeat(70));
            for book in books {
                let author = book.author_sort.as_deref().unwrap_or("Unknown");
                let title = if book.title.len() > 37 {
                    format!("{}...", &book.title[..37])
                } else {
                    book.title.clone()
                };
                println!("{:<5} {:<40} {:<20}", book.id, title, author);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn test_cmd_list() {
        let mut db = Library::open_test().unwrap();
        db.insert_test_book("Test Book 1").unwrap();
        db.insert_test_book("Test Book 2").unwrap();

        let cmd = CmdList::new();
        // Test basic list
        let args = vec![];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());

        // Test search
        let args = vec!["--search".to_string(), "Book 1".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());

        // Test limit
        let args = vec!["--limit".to_string(), "1".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());
    }
}
