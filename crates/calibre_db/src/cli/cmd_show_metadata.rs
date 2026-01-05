use crate::Library;
use anyhow::{Context, Result};

pub struct CmdShowMetadata;

impl CmdShowMetadata {
    pub fn new() -> Self {
        CmdShowMetadata
    }

    pub fn run(&self, db: &Library, args: &[String]) -> Result<()> {
        let mut as_opf = false;
        let mut book_id_opt = None;

        for arg in args {
            if arg == "--as-opf" {
                as_opf = true;
            } else if !arg.starts_with('-') {
                if let Ok(id) = arg.parse::<i32>() {
                    book_id_opt = Some(id);
                }
            }
        }

        let book_id = book_id_opt.context("You must specify an id")?;

        let book = db
            .get_book(book_id)?
            .context(format!("Id #{} is not present in database.", book_id))?;

        if as_opf {
            println!("TODO: --as-opf support not yet completely ported. Showing text instead.");
            println!("{:#?}", book);
        } else {
            // Mimic python's basic str(mi) which usually shows title, author etc.
            // For now, Debug/Display is fine.
            println!("Title: {}", book.title);
            println!(
                "Author(s): {}",
                book.author_sort.as_deref().unwrap_or("Unknown")
            );
            // Add more fields as needed to match Python output
            println!("Path: {}", book.path);
            if book.has_cover {
                println!("Cover: Yes");
            } else {
                println!("Cover: No");
            }
            println!("Formats: {:?}", db.get_default_book_file(&book)); // Simple format check
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn test_cmd_show_metadata() {
        let mut db = Library::open_test().unwrap();
        db.insert_test_book("Test Book 1").unwrap();
        let book_id = 1; // Assuming first insert is 1

        let cmd = CmdShowMetadata::new();

        let args = vec![book_id.to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());
    }
}
