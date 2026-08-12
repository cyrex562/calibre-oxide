use crate::Library;
use anyhow::Result;
use std::io::{self, Write};

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

        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let _ = write_check_library_results(&mut handle, &all_checks(&checker), csv);

        Ok(())
    }
}

/// Assemble the ordered `(label, results)` tuples the printer walks.
/// Extracted so tests can also enumerate the full check set without
/// duplicating the list.
pub fn all_checks<'a>(
    checker: &'a crate::check_library::CheckLibrary,
) -> Vec<(&'static str, &'a [(String, String, i32)])> {
    vec![
        ("Invalid titles", &checker.invalid_titles),
        ("Extra titles", &checker.extra_titles),
        ("Invalid authors", &checker.invalid_authors),
        ("Extra authors", &checker.extra_authors),
        ("Missing book formats", &checker.missing_formats),
        ("Extra book formats", &checker.extra_formats),
        ("Unknown files in books", &checker.extra_files),
        ("Missing cover files", &checker.missing_covers),
        ("Cover files not in database", &checker.extra_covers),
        ("Malformed formats", &checker.malformed_formats),
        ("Malformed book paths", &checker.malformed_paths),
    ]
}

/// Port of the Python `_print_check_library_results` in
/// `old_src/src/calibre/db/cli/tests.py`.
///
/// Writes to any `Write` sink (so tests can capture into a buffer).
/// An empty `results` list emits nothing at all — the caller does not
/// need to pre-filter, matching Python's behavior tested by
/// `test_prints_nothing_if_no_errors`.
pub fn write_check_library_results<W: Write>(
    out: &mut W,
    checks: &[(&'static str, &[(String, String, i32)])],
    as_csv: bool,
) -> io::Result<()> {
    for (label, list) in checks {
        if list.is_empty() {
            continue;
        }
        if as_csv {
            for (param1, param2, _id) in list.iter() {
                let mut wtr = csv::WriterBuilder::new()
                    .quote_style(csv::QuoteStyle::Necessary)
                    .flexible(true)
                    .from_writer(&mut *out);
                wtr.write_record([*label, param1.as_str(), param2.as_str()])?;
                wtr.flush()?;
            }
        } else {
            writeln!(out, "{}", label)?;
            for (param1, param2, _id) in list.iter() {
                writeln!(out, "    {: <40} - {: <40}", param1, param2)?;
            }
        }
    }
    Ok(())
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

    // Port of PrintCheckLibraryResultsTest from
    // old_src/src/calibre/db/cli/tests.py. Each Rust test mirrors the
    // Python test of the same name.

    const LABEL: &str = "Dummy Check";

    fn make_check(list: &[(String, String, i32)]) -> Vec<(&'static str, &[(String, String, i32)])> {
        vec![(LABEL, list)]
    }

    #[test]
    fn prints_nothing_if_no_errors() {
        let empty: Vec<(String, String, i32)> = Vec::new();
        let checks = make_check(&empty);
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, false).unwrap();
        assert_eq!(buf, b"");
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, true).unwrap();
        assert_eq!(buf, b"");
    }

    #[test]
    fn human_readable_output() {
        let data = vec![("first".to_string(), "second".to_string(), 0)];
        let checks = make_check(&data);
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, false).unwrap();

        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        // data rows + 1 header + 1 trailing empty (post-final-\n split).
        assert_eq!(lines.len(), data.len() + 2, "lines = {:?}", lines);
        assert_eq!(lines[0], LABEL);

        let mut parts = lines[1].split('-');
        let first = parts.next().unwrap().trim();
        let second = parts.next().unwrap().trim();
        assert_eq!(first, "first");
        assert_eq!(second, "second");

        assert_eq!(lines[lines.len() - 1], "");
    }

    #[test]
    fn basic_csv_output() {
        let data = vec![("first".to_string(), "second".to_string(), 0)];
        let checks = make_check(&data);
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, true).unwrap();

        let text = String::from_utf8(buf).unwrap();
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes());
        let records: Vec<Vec<String>> = rdr
            .records()
            .map(|r| r.unwrap().iter().map(str::to_string).collect())
            .collect();
        assert_eq!(records, vec![vec![LABEL.to_string(), "first".to_string(), "second".to_string()]]);
    }

    #[test]
    fn escaped_csv_output() {
        // Python test uses "I, Caesar" — contains a comma so must be
        // quoted. The escape must round-trip.
        let data = vec![("I, Caesar".to_string(), "second".to_string(), 0)];
        let checks = make_check(&data);
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, true).unwrap();

        let text = String::from_utf8(buf).unwrap();
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes());
        let records: Vec<Vec<String>> = rdr
            .records()
            .map(|r| r.unwrap().iter().map(str::to_string).collect())
            .collect();
        assert_eq!(
            records,
            vec![vec![LABEL.to_string(), "I, Caesar".to_string(), "second".to_string()]]
        );
    }

    #[test]
    fn skips_only_empty_checks_when_mixed_with_populated() {
        // Not in the Python original but a valuable regression guard:
        // an empty check inside a list of populated checks must not
        // print its label as a "false positive".
        let empty: Vec<(String, String, i32)> = Vec::new();
        let populated = vec![("t".to_string(), "u".to_string(), 0)];
        let checks: Vec<(&'static str, &[(String, String, i32)])> = vec![
            ("Empty Check", &empty),
            ("Populated Check", &populated),
        ];
        let mut buf: Vec<u8> = Vec::new();
        write_check_library_results(&mut buf, &checks, false).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains("Empty Check"), "output = {:?}", text);
        assert!(text.contains("Populated Check"), "output = {:?}", text);
    }
}
