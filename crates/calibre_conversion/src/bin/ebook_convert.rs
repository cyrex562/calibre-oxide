use anyhow::{Context, Result};
use calibre_conversion::plugins::epub_input::EpubInput;
use calibre_conversion::plugins::epub_output::EpubOutput;
use calibre_conversion::transform::html_roundtrip::HtmlRoundTrip;
use calibre_conversion::{ConversionOptions, ConversionPipeline};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Input file path
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Output file path
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    println!("Converting {:?} to {:?}", cli.input, cli.output);

    // TODO: Dynamic plugin selection based on extension.
    // For now, hardcoded to EPUB -> EPUB.

    // Check extensions
    let input_ext = cli.input.extension().and_then(|s| s.to_str()).unwrap_or("");
    let output_ext = cli
        .output
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if !input_ext.eq_ignore_ascii_case("epub") {
        anyhow::bail!("Only EPUB input is supported currently.");
    }
    if !output_ext.eq_ignore_ascii_case("epub") {
        anyhow::bail!("Only EPUB output is supported currently.");
    }

    // Setup Pipeline
    let input_plugin = Box::new(EpubInput);
    let output_plugin = Box::new(EpubOutput);
    let mut pipeline = ConversionPipeline::new(input_plugin, output_plugin);

    // Add Default Transforms
    // To demonstrate processing we use the HtmlRoundTrip transform
    pipeline.add_transform(Box::new(HtmlRoundTrip));

    // Run
    let options = ConversionOptions::default();
    pipeline.run(&cli.input, &cli.output, &options)?;

    println!("Conversion complete!");
    Ok(())
}
