use clap::Parser;

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// IDs to export
    pub ids: Vec<String>,

    #[clap(long)]
    pub all: bool,

    #[clap(long, default_value = ".")]
    pub to_dir: String,

    #[clap(long)]
    pub single_dir: bool,

    #[clap(long)]
    pub progress: bool,
}

pub struct CmdExport;

impl CmdExport {
    pub fn new() -> Self {
        CmdExport
    }

    pub fn run(&self, db: &crate::Library, args: &RunArgs) -> anyhow::Result<()> {
        let root_dir = std::path::PathBuf::from(&args.to_dir);
        if !root_dir.exists() {
            std::fs::create_dir_all(&root_dir)?;
        }

        let ids = if args.all {
            db.all_book_ids()?
        } else {
            args.ids
                .iter()
                .filter_map(|s| s.parse::<i32>().ok())
                .collect()
        };

        for (_i, id) in ids.iter().enumerate() {
            if let Some(book) = db.get_book(*id)? {
                if let Some(src_path) = db.get_default_book_file(&book) {
                    // Determine destination filename
                    // Ideally use templating, but for now: Author - Title.ext
                    let ext = src_path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    let safe_title = calibre_utils::filenames::sanitize_file_name(&book.title);
                    let safe_author = calibre_utils::filenames::sanitize_file_name(
                        book.author_sort.as_deref().unwrap_or("Unknown"),
                    );

                    let filename = format!("{} - {}.{}", safe_author, safe_title, ext);

                    let dest_path = if args.single_dir {
                        root_dir.join(filename)
                    } else {
                        // Author/Title.ext structure
                        let author_dir = root_dir.join(&safe_author);
                        if !author_dir.exists() {
                            std::fs::create_dir_all(&author_dir)?;
                        }
                        author_dir.join(filename)
                    };

                    std::fs::copy(&src_path, &dest_path)?;
                    if args.progress {
                        println!("Exported: {} -> {:?}", book.title, dest_path);
                    }
                } else if args.progress {
                    println!("Skipping book {}: No file found", book.title);
                }
            }
        }

        Ok(())
    }
}
