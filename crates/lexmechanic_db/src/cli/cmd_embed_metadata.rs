use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// List of book ids
    #[arg(required = true, value_delimiter = ',', num_args = 1..)]
    pub ids: Vec<String>,
}

pub struct CmdEmbedMetadata;

impl CmdEmbedMetadata {
    pub fn new() -> Self {
        CmdEmbedMetadata
    }

    pub fn run(&self, db: &crate::Library, args: &RunArgs) -> Result<()> {
        let ids = if args.ids.contains(&"all".to_string()) {
            db.all_book_ids()?
        } else {
            args.ids
                .iter()
                .filter_map(|s| s.parse::<i32>().ok())
                .collect()
        };

        let cache = db.as_cache();
        for id in ids {
            // The OPF sidecar is still written: it is what `restore`
            // rebuilds from, and it carries fields no book format has a
            // place for.
            db.backup_metadata_to_opf(id)?;

            // And now the real thing (#834). Formats with no writer yet
            // are named as skipped rather than passed over in silence --
            // reporting "Processed book id: 4" while touching none of its
            // files is what made the old behaviour misleading.
            match crate::embed::embed_metadata(&cache, id) {
                Ok(outcomes) if outcomes.is_empty() => println!("Book {id}: no formats to embed into"),
                Ok(outcomes) => {
                    for (format, outcome) in outcomes {
                        match outcome {
                            crate::embed::FormatOutcome::Embedded => println!("Book {id}: embedded metadata into {format}"),
                            crate::embed::FormatOutcome::NoWriter => println!("Book {id}: {format} skipped -- no metadata writer for it yet"),
                            crate::embed::FormatOutcome::FileMissing => println!("Book {id}: {format} skipped -- the file is missing"),
                            crate::embed::FormatOutcome::Failed(why) => println!("Book {id}: {format} failed -- {why}"),
                        }
                    }
                }
                Err(e) => println!("Book {id}: could not embed metadata -- {e:#}"),
            }
        }
        Ok(())
    }
}
