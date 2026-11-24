//! Port of `old_src/src/calibre/ebooks/pdf/develop.py` (42 lines).
//!
//! A Qt+`podofo`-dependent CLI dev/debug tool for interactively testing
//! HTML-to-PDF rendering (`Renderer(QWebEnginePage)` loads a file, prints
//! to PDF, then round-trips the bytes through `podofo` to sanity-check the
//! output). No format-support value of its own - entirely dependent on
//! [`crate::pdf::html_writer`]'s Qt rendering core, which is this port's
//! documented gap. Kept as a small real-signature stub rather than a full
//! port: this is low-value interactive tooling, not core format support.

/// Port of `main()` (develop.py lines 31-38): load `input_path` in a
/// (would-be) `QWebEnginePage`, print it to PDF, and write the result to
/// `output_path`.
pub fn main(_input_path: &std::path::Path, _output_path: &std::path::Path) {
    todo!(
        "placeholder: develop.py is a Qt+podofo interactive dev tool with no format-support value \
         of its own - entirely dependent on crate::pdf::html_writer's Qt QWebEnginePage rendering \
         core, which is this port's documented gap (see html_writer.rs's module doc comment)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "placeholder")]
    fn main_is_a_documented_gap() {
        main(
            std::path::Path::new("in.html"),
            std::path::Path::new("out.pdf"),
        );
    }
}
