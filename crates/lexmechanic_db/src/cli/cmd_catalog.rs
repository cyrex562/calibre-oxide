use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Output file
    pub output_file: PathBuf,

    #[clap(long, short = 'i')]
    pub ids: Option<String>,

    #[clap(long, short = 's')]
    pub search: Option<String>,

    #[clap(long, short = 'v')]
    pub verbose: bool,
}

pub struct CmdCatalog {
    // Empty
}

impl CmdCatalog {
    pub fn new() -> Self {
        CmdCatalog {}
    }

    pub fn run(&self, db: &crate::Library, args: &RunArgs) -> anyhow::Result<()> {
        let books = if let Some(search) = &args.search {
            let ids = db.search(search)?;
            let mut book_list = Vec::new();
            for id in ids {
                if let Some(book) = db.get_book(id)? {
                    book_list.push(book);
                }
            }
            book_list
        } else if let Some(ids_str) = &args.ids {
            let mut book_list = Vec::new();
            for id_str in ids_str.split(',') {
                if let Ok(id) = id_str.trim().parse::<i32>() {
                    if let Some(book) = db.get_book(id)? {
                        book_list.push(book);
                    }
                }
            }
            book_list
        } else {
            db.list_books()?
        };

        // Determine format from extension
        let ext = args
            .output_file
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("csv");

        if ext.eq_ignore_ascii_case("csv") {
            let mut wtr = csv::Writer::from_path(&args.output_file)?;
            // Write header
            wtr.write_record(&["Title", "Author", "Date", "ISBN", "Path"])?;

            for book in books {
                wtr.write_record(&[
                    &book.title,
                    &book.author_sort.clone().unwrap_or_default(),
                    &book.pubdate.clone().unwrap_or_default(),
                    &book.isbn.clone().unwrap_or_default(),
                    &book.path,
                ])?;
            }
            wtr.flush()?;

            if args.verbose {
                println!("Catalog written to {:?}", args.output_file);
            }
        } else {
            return Err(anyhow::anyhow!(
                "Unsupported catalog format: {}. Only CSV is supported currently.",
                ext
            ));
        }

        Ok(())
    }
}
