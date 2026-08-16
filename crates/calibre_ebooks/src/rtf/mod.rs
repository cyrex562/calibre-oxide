//! Port of `old_src/src/calibre/ebooks/rtf/{input,preprocess,rtfml}.py`
//! (`__init__.py` is empty upstream -- nothing to port there).
//!
//! - `preprocess.rs`: port of `preprocess.py` (`RtfTokenizer`,
//!   `RtfTokenParser`) -- a self-contained RTF tokenizer/token-parser,
//!   pure state-machine logic with no external dependencies.
//! - `rtfml.rs`: port of `rtfml.py` (`RTFMLizer`) -- OEB/XHTML -> RTF
//!   markup, the module `output::rtf_output::RTFOutput` now drives.
//! - `input.rs`: port of `input.py` (`InlineClass`) as
//!   [`input::InlineClassAssigner`] -- see that module's doc comment
//!   for why this is real logic with no current caller (the XSLT
//!   pipeline it plugged into upstream, `calibre.ebooks.rtf2xml.*`, is
//!   a separate, out-of-scope subsystem this crate does not implement;
//!   see `crate::input::rtf_input`'s own doc comment).

pub mod input;
pub mod preprocess;
pub mod rtfml;
