use anyhow::{Context, Result};
use calibre_ebooks::metadata::{get_metadata, MetaInformation};
use clap::Parser;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ebook-meta")]
#[command(about = "Read/Write metadata from/to e-book files", long_about = None)]
struct Args {
    /// Input e-book file
    #[arg(required = true)]
    input_file: PathBuf,

    /// Specify the name of an OPF file. The metadata will be written to the OPF file.
    #[arg(long)]
    to_opf: Option<PathBuf>,

    /// Get the cover from the e-book and save it as the specified file.
    #[arg(long)]
    get_cover: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.input_file.exists() {
        eprintln!("Error: Input file does not exist: {:?}", args.input_file);
        std::process::exit(1);
    }

    // Extract metadata
    println!("Reading metadata from: {:?}", args.input_file);
    let mi = get_metadata(&args.input_file).context("Failed to read metadata")?;

    // Print Metadata to stdout (Mimic legacy output style)
    print_metadata(&mi);

    // Handle --to-opf
    if let Some(opf_path) = args.to_opf {
        println!("Writing OPF to: {:?}", opf_path);
        let xml = mi.to_xml();
        let mut file = File::create(&opf_path).context("Failed to create OPF file")?;
        file.write_all(xml.as_bytes())
            .context("Failed to write to OPF file")?;
        println!("OPF created in {:?}", opf_path);
    }

    // Handle --get-cover
    if let Some(cover_path) = args.get_cover {
        if let (Some(ext), ref data) = mi.cover_data {
            if !data.is_empty() {
                println!("Saving cover ({}) to: {:?}", ext, cover_path);
                let mut file = File::create(&cover_path).context("Failed to create cover file")?;
                file.write_all(data).context("Failed to write cover data")?;
                println!("Cover saved to {:?}", cover_path);
            } else {
                eprintln!("No cover data found in ebook.");
            }
        } else {
            eprintln!("No cover found in ebook.");
        }
    }

    Ok(())
}

fn print_metadata(mi: &MetaInformation) {
    println!("Title               : {}", mi.title);
    if !mi.authors.is_empty() {
        println!("Author(s)           : {}", mi.authors.join(" & "));
    }
    if let Some(publisher) = &mi.publisher {
        println!("Publisher           : {}", publisher);
    }
    if !mi.languages.is_empty() {
        println!("Languages           : {}", mi.languages.join(", "));
    }
    if let Some(pubdate) = &mi.pubdate {
        println!("Published           : {}", pubdate);
    }
    if !mi.tags.is_empty() {
        println!("Tags                : {}", mi.tags.join(", "));
    }
    if let Some(series) = &mi.series {
        println!("Series              : {} #{}", series, mi.series_index);
    }
    if !mi.identifiers.is_empty() {
        let ids: Vec<String> = mi
            .identifiers
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect();
        println!("Identifiers         : {}", ids.join(", "));
    }
    if let Some(comments) = &mi.comments {
        println!("Comments            : {}", comments);
    }
}
