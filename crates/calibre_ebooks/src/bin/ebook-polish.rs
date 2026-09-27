//! `ebook-polish` — the console front end to `oeb::polish::main::polish_one` (#813).
//!
//! The engine has been merged and tested since #169; until now its only
//! caller was `POST /polish` in the content server, so there was no way
//! to polish a file from a shell. This is the wrapper, not new
//! functionality.
//!
//! Two narrowings are inherited from the engine and disclosed rather than
//! hidden: `--opf` and the font `--embed`/`--subset` options all reach
//! real documented gaps in `polish_one`, so they are not offered here.
//! Offering a flag that errors would be worse than not offering it.

use anyhow::{bail, Context, Result};
use calibre_ebooks::oeb::polish::container::{AnyContainer, EpubContainer};
use calibre_ebooks::oeb::polish::main::{polish_one, PolishCustomization, PolishOptions};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ebook-polish")]
#[command(about = "Polish an EPUB in place: covers, jackets, punctuation, unused CSS", long_about = None)]
struct Args {
    /// The EPUB to polish.
    #[arg(required = true)]
    input_file: PathBuf,

    /// Where to write the result. Defaults to polishing in place.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Replace the cover with this image.
    #[arg(long, value_name = "IMAGE")]
    cover: Option<String>,

    /// Add a "book jacket" page of metadata at the start.
    #[arg(long)]
    jacket: bool,

    /// Remove any existing book jacket.
    #[arg(long)]
    remove_jacket: bool,

    /// Convert quotes, dashes and ellipses to their typographic forms.
    #[arg(long)]
    smarten_punctuation: bool,

    /// Remove CSS rules nothing in the book matches.
    #[arg(long)]
    remove_unused_css: bool,

    /// Recompress images losslessly.
    #[arg(long)]
    compress_images: bool,

    /// Upgrade the book's internals to the newest supported EPUB version.
    #[arg(long)]
    upgrade_book: bool,

    /// Insert soft hyphens to improve justification.
    #[arg(long)]
    add_soft_hyphens: bool,

    /// Remove soft hyphens.
    #[arg(long)]
    remove_soft_hyphens: bool,

    /// Download images and stylesheets the book links to remotely.
    #[arg(long)]
    download_external_resources: bool,

    /// Report what would change without writing anything.
    #[arg(long)]
    dry_run: bool,
}

impl Args {
    fn to_options(&self) -> PolishOptions {
        PolishOptions {
            cover: self.cover.clone(),
            // Deliberately not exposed -- `polish_one` errors on it.
            opf: None,
            jacket: self.jacket,
            remove_jacket: self.remove_jacket,
            smarten_punctuation: self.smarten_punctuation,
            remove_unused_css: self.remove_unused_css,
            compress_images: self.compress_images,
            upgrade_book: self.upgrade_book,
            add_soft_hyphens: self.add_soft_hyphens,
            remove_soft_hyphens: self.remove_soft_hyphens,
            download_external_resources: self.download_external_resources,
            // Both reach documented `todo!()` gaps in the engine.
            embed: false,
            subset: false,
        }
    }

    /// Whether any polishing was actually requested.
    ///
    /// Checked because `polish_one` with nothing enabled succeeds and
    /// changes nothing, so the tool would silently appear to work while
    /// doing no work at all.
    fn asks_for_anything(&self) -> bool {
        self.cover.is_some()
            || self.jacket
            || self.remove_jacket
            || self.smarten_punctuation
            || self.remove_unused_css
            || self.compress_images
            || self.upgrade_book
            || self.add_soft_hyphens
            || self.remove_soft_hyphens
            || self.download_external_resources
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.input_file.is_file() {
        bail!("{} is not a file", args.input_file.display());
    }
    if !args.asks_for_anything() {
        bail!("nothing to do: pass at least one option, e.g. --smarten-punctuation (see --help)");
    }

    let tdir = tempfile::tempdir()?;
    let mut container = AnyContainer::Epub(EpubContainer::open_zip(&args.input_file, tdir.path()).with_context(|| format!("opening {}", args.input_file.display()))?);

    let opts = args.to_options();
    let customization = PolishCustomization::default();

    // The engine reports progress as free text; a console tool's job is
    // to show it rather than collect it.
    let mut report = |line: &str| {
        let line = line.trim();
        if !line.is_empty() {
            println!("{line}");
        }
    };
    let changed = polish_one(&mut container, &opts, &mut report, Some(&customization))?;

    if !changed {
        println!("Nothing needed changing.");
        return Ok(());
    }
    if args.dry_run {
        println!("--dry-run: the book was not written.");
        return Ok(());
    }

    // Committed to a temp file and then moved into place, so an
    // interrupted write cannot leave a half-polished book where the
    // original was.
    let staging = tempfile::tempdir()?;
    let staged = staging.path().join("polished.epub");
    match &mut container {
        AnyContainer::Epub(c) => c.commit(Some(&staged))?,
        _ => bail!("only EPUB files can be polished"),
    }

    let destination = args.output.clone().unwrap_or_else(|| args.input_file.clone());
    // `rename` fails across filesystems, which a temp dir very often is.
    std::fs::copy(&staged, &destination).with_context(|| format!("writing {}", destination.display()))?;
    println!("Wrote {}", destination.display());
    Ok(())
}
