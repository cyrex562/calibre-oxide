//! `ebook-convert` CLI. Real argument validation via
//! [`calibre_conversion::cli_helpers::check_command_line_options`]
//! (matching upstream's `.EXT` output-shorthand and `.recipe`
//! readability exemption), dispatching to
//! [`calibre_ebooks::conversion::plumber::Plumber`] -- the crate's
//! real, tested, per-format input/output dispatch table (see
//! `calibre_conversion`'s own crate-root doc, issue #476, for why this
//! binary no longer has its own separate hardcoded-EPUB-only pipeline).

use anyhow::{Context, Result};
use calibre_conversion::cli_helpers::{check_command_line_options, CliArgError, USAGE_BANNER};
use calibre_ebooks::conversion::plumber::Plumber;

fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();

    let (input, output) = match check_command_line_options(&args, |p| p.is_file()) {
        Ok(io) => io,
        Err(CliArgError::MissingIoArgs) => {
            eprintln!("{USAGE_BANNER}");
            anyhow::bail!(CliArgError::MissingIoArgs);
        }
        Err(e) => return Err(e.into()),
    };

    println!("Converting {input:?} to {output:?}");
    Plumber::new(&input, &output).run().with_context(|| format!("failed to convert {input:?} to {output:?}"))?;
    println!("Conversion complete!");
    Ok(())
}
