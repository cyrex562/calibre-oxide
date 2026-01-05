use crate::Library;
use anyhow::Result;

pub struct CmdBackupMetadata;

impl CmdBackupMetadata {
    pub fn new() -> Self {
        CmdBackupMetadata
    }

    pub fn run(&self, db: &Library, args: &[String]) -> Result<()> {
        let mut force_all = false;
        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--all" => {
                    force_all = true;
                }
                _ => {}
            }
            idx += 1;
        }

        let book_ids = if force_all {
            db.all_book_ids()?
        } else {
            // In a full implementation, we'd check for dirty books.
            // For this port, we'll default to all if nothing specified or just do nothing?
            // Python default is "dirty only".
            // Since we don't have dirty tracking yet, let's just do all if --all is passed, otherwise maybe none or warn?
            // But usually the user runs this manually to force update.
            // Let's assume for now if they run it they want something to happen.
            // But strict port says "normally only operates on books that have out of date OPF files".
            // "This option (--all) makes it operate on all books."
            // So if no --all, and no dirty tracking, we do nothing?
            // Let's output a message if no --all is passed saying "Dirty tracking not implemented, use --all to backup all books."
            println!(
                "Note: Dirty tracking is not implemented. Use --all to force backup of all books."
            );
            return Ok(());
        };

        println!("Backing up metadata for {} books...", book_ids.len());
        for (i, id) in book_ids.iter().enumerate() {
            if i % 100 == 0 {
                println!("Processed {}/{}...", i, book_ids.len());
            }
            if let Err(e) = db.backup_metadata_to_opf(*id) {
                eprintln!("Failed to backup book {}: {}", id, e);
            }
        }
        println!("Backup complete.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn test_cmd_backup_metadata() {
        let mut db = Library::open_test().unwrap();
        // Insert a book causing a file creation

        // This confirms the command runs successfully even if it does nothing
        db.insert_test_book("Test Book").unwrap();

        let cmd = CmdBackupMetadata::new();
        let args = vec!["--all".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());
    }
}
