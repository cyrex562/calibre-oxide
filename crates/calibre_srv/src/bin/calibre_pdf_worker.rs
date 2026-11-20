//! The PDF worker process: reads PDFs for the library jobs so PDFium's
//! crashes and hangs cannot take the server down. One JSON request per line on
//! stdin, one JSON response per line on stdout; see `calibre_db::pdf_worker`
//! for the protocol and for how the server finds and runs this.

fn main() {
    if let Err(e) = calibre_db::pdf_worker::serve_process(calibre_db::pdf_worker::handle) {
        eprintln!("calibre_pdf_worker: {e}");
        std::process::exit(1);
    }
}
