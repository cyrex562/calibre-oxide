//! The PDF worker process: reads PDFs for the library jobs so PDFium's
//! crashes and hangs cannot take the server down. One JSON request per line on
//! stdin, one JSON response per line on stdout; see `lexmechanic_db::pdf_worker`
//! for the protocol and for how the server finds and runs this.

fn main() {
    if let Err(e) = lexmechanic_db::pdf_worker::serve_process(lexmechanic_db::pdf_worker::handle) {
        eprintln!("lexmechanic_pdf_worker: {e}");
        std::process::exit(1);
    }
}
