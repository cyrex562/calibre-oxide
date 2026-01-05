use crate::Library;
use anyhow::Result;

pub struct CmdCheckLibrary;

impl CmdCheckLibrary {
    pub fn new() -> Self {
        CmdCheckLibrary
    }

    pub fn run(&self, db: &Library, args: &[String]) -> Result<()> {
        let mut vacuum = false;
        let mut idx = 0;
        let mut csv = false;
        // Basic argument parsing
        while idx < args.len() {
            if args[idx] == "--vacuum-fts-db" {
                vacuum = true;
            } else if args[idx] == "--csv" || args[idx] == "-c" {
                csv = true;
            }
            idx += 1;
        }

        if vacuum {
            println!("Vacuuming database...");
            db.vacuum(true)?;
        }

        let path = db.path();
        let mut checker = crate::check_library::CheckLibrary::new(path.to_path_buf(), db);
        checker.scan_library(vec![], vec![]);

        self.print_results(&checker, csv);

        Ok(())
    }

    fn print_results(&self, checker: &crate::check_library::CheckLibrary, csv: bool) {
        let checks = vec![
            ("invalid_titles", "Invalid titles", &checker.invalid_titles),
            ("extra_titles", "Extra titles", &checker.extra_titles),
            (
                "invalid_authors",
                "Invalid authors",
                &checker.invalid_authors,
            ),
            ("extra_authors", "Extra authors", &checker.extra_authors),
            (
                "missing_formats",
                "Missing book formats",
                &checker.missing_formats,
            ),
            (
                "extra_formats",
                "Extra book formats",
                &checker.extra_formats,
            ),
            (
                "extra_files",
                "Unknown files in books",
                &checker.extra_files,
            ),
            (
                "missing_covers",
                "Missing cover files",
                &checker.missing_covers,
            ),
            (
                "extra_covers",
                "Cover files not in database",
                &checker.extra_covers,
            ),
            (
                "malformed_formats",
                "Malformed formats",
                &checker.malformed_formats,
            ),
            (
                "malformed_paths",
                "Malformed book paths",
                &checker.malformed_paths,
            ),
            // ("failed_folders", "Folders raising exception", &checker.failed_folders), // Different type signature
        ];

        for (_key, label, list) in checks {
            if list.is_empty() {
                continue;
            }
            if csv {
                for (param1, param2, _id) in list {
                    println!("{},{},{}", label, param1, param2);
                }
            } else {
                println!("{}", label);
                for (param1, param2, _id) in list {
                    println!("    {: <40} - {: <40}", param1, param2);
                }
            }
        }

        if !checker.failed_folders.is_empty() {
            println!("Folders raising exception");
            for (path, err, _) in &checker.failed_folders {
                println!("    {: <40} - {}", path, err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn test_cmd_check_library() {
        let db = Library::open_test().unwrap();
        let cmd = CmdCheckLibrary::new();
        let args = vec![];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());

        let args = vec!["--vacuum-fts-db".to_string()];
        let res = cmd.run(&db, &args);
        assert!(res.is_ok());
    }
}
