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

        for id in ids {
            // In the Python code, this calls `db.embed_metadata`.
            // Currently Library::embed_metadata isn't fully separate, but `backup_metadata_to_opf`
            // is the closest equivalent for updating the OPF file.
            // Real embedding into EPUB/MOBI etc would require `calibre_ebooks` support for writing those formats with metadata.
            // For now, we update the OPF which is a critical part of "embedding" in terms of saving metadata to disk.
            db.backup_metadata_to_opf(id)?;
            println!("Processed book id: {}", id);
        }
        Ok(())
    }
}
