use crate::Library;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct CmdAddFormat;

impl CmdAddFormat {
    pub fn new() -> Self {
        CmdAddFormat
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        let mut replace = true;
        let mut book_id = None;
        let mut file_path = None;

        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--dont-replace" => {
                    replace = false;
                }
                arg => {
                    if book_id.is_none() {
                        book_id = Some(arg.parse::<i32>().context("Invalid book ID")?);
                    } else if file_path.is_none() {
                        file_path = Some(PathBuf::from(arg));
                    }
                }
            }
            idx += 1;
        }

        let book_id = book_id.context("Internal Error: Book ID required")?;
        let file_path = file_path.context("Internal Error: File path required")?;

        if !file_path.exists() {
            anyhow::bail!("File not found: {:?}", file_path);
        }

        let extension = file_path
            .extension()
            .and_then(|e| e.to_str())
            .context("File has no extension")?;

        let added = db.add_format(book_id, &file_path, extension, replace)?;

        if added {
            println!("Added {} format to book {}", extension, book_id);
        } else {
            println!("Format {} already exists, not replacing.", extension);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_cmd_add_format() {
        let mut db = Library::open_test().unwrap();
        // `add_format` requires the book to already have a folder
        // (`insert_test_book`'s default empty path won't do), so
        // insert one directly with a real relative path.
        db.conn()
            .execute(
                "INSERT INTO books (title, path) VALUES ('Test Book', 'Author/Test Book')",
                [],
            )
            .unwrap();
        let book_id = db.conn().last_insert_rowid() as i32;

        let cmd = CmdAddFormat::new();

        let file_path = db.path().join("source.epub");
        let mut f = fs::File::create(&file_path).unwrap();
        f.write_all(b"dummy content").unwrap();

        let args = vec![book_id.to_string(), file_path.to_string_lossy().to_string()];
        cmd.run(&mut db, &args).unwrap();

        let dest = db.path().join("Author/Test Book/Test Book.epub");
        assert!(
            dest.exists(),
            "format file should be copied into the book's folder"
        );
        assert_eq!(fs::read_to_string(dest).unwrap(), "dummy content");
    }
}
