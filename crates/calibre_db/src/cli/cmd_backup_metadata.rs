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
        let mut written = 0usize;
        let mut failed = 0usize;
        for (i, id) in book_ids.iter().enumerate() {
            if i > 0 && i % 100 == 0 {
                println!("Processed {}/{}...", i, book_ids.len());
            }
            match db.backup_metadata_to_opf(*id) {
                Ok(()) => written += 1,
                Err(e) => {
                    failed += 1;
                    eprintln!("Failed to backup book {}: {}", id, e);
                }
            }
        }

        // Report what was actually written. This used to print
        // "Backup complete." unconditionally, which meant a library
        // where every single book was skipped -- the case for every
        // library in #889's layout, since `backup_metadata_to_opf`
        // returned early for a book with an empty `path` -- still
        // reported success with no backup on disk at all. A
        // data-protection command must not claim to have done
        // something it did not do.
        if failed > 0 {
            println!("Backed up {written} of {} books; {failed} failed.", book_ids.len());
        } else {
            println!("Backed up {written} books.");
        }
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
