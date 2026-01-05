use calibre_ebooks::conversion::plumber::Plumber;
use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: ebook-convert <input_file> <output_dir>");
        process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    let plumber = Plumber::new(input_path, output_path);
    if let Err(e) = plumber.run() {
        eprintln!("Conversion Error: {}", e);
        process::exit(1); // Exit with error code
    }
}
