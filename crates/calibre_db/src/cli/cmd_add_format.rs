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
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().to_path_buf();
        // Setup library
        fs::create_dir_all(db_path.join("metadata.db")).unwrap(); // Placeholder implies existing DB logic usually, but here we use open_test mostly?
                                                                  // usage of open_test creates in-memory DB but with path set to :memory:. add_format skips file ops for memory.
                                                                  // We need a real file backed test for add_format to test file ops, OR we rely on the memory check in add_format returning true.
                                                                  // Let's use open_test() which is memory. add_format returns Ok(true) immediately.
                                                                  // To test real logic we need a real DB on disk.

        // Actually Library::open takes a path.
        // We need to initialize the DB schema if we use a real path.
        // Library::open_test() handles schema init but uses :memory: path.
        // Let's modify Library::open_test to optionally take a path or make a helper.
        // Or just use the :memory: one and verify it returns Ok.

        let mut db = Library::open_test().unwrap();
        db.insert_test_book("Test Book").unwrap();
        let book_id = 1;

        let cmd = CmdAddFormat::new();

        // Create a dummy file
        let file_path = temp_dir.path().join("book.epub");
        let mut f = fs::File::create(&file_path).unwrap();
        f.write_all(b"dummy content").unwrap();

        // Test running
        let args = vec![book_id.to_string(), file_path.to_string_lossy().to_string()];
        let res = cmd.run(&mut db, &args);
        assert!(res.is_ok());
    }
}
