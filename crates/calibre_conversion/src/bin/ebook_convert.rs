//! `ebook-convert` CLI. Real argument validation via
//! [`calibre_conversion::cli_helpers::check_command_line_options`]
//! (matching upstream's `.EXT` output-shorthand and `.recipe`
//! readability exemption), real option parsing via
//! [`calibre_conversion::cli_options::ConvertArgs`] (issue #126),
//! dispatching to [`calibre_ebooks::conversion::plumber::Plumber`] --
//! the crate's real, tested, per-format input/output dispatch table
//! (see `calibre_conversion`'s own crate-root doc, issue #476, for why
//! this binary no longer has its own separate hardcoded-EPUB-only
//! pipeline).

use anyhow::{Context, Result};
use calibre_conversion::cli_helpers::{check_command_line_options, CliArgError, USAGE_BANNER};
use calibre_conversion::cli_options::ConvertArgs;
use calibre_ebooks::conversion::plumber::Plumber;
use clap::Parser;

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

    // Positional input/output already consumed above (args[1]/args[2]);
    // everything from args[3] onward is real conversion options.
    let convert_args = ConvertArgs::parse_from(std::iter::once("ebook-convert".to_string()).chain(args.iter().skip(3).cloned()));
    let opts = convert_args.into_conversion_options();

    println!("Converting {input:?} to {output:?}");
    Plumber::with_options(&input, &output, opts).run().with_context(|| format!("failed to convert {input:?} to {output:?}"))?;
    println!("Conversion complete!");
    Ok(())
}
