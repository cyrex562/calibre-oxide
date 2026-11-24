//! The KF8 writer: encodes an OEB book into the KF8 payload embedded in a
//! `.azw3` file (skeleton+chunk XHTML splitting, CSS/SVG flow records,
//! native `SKEL`/`CHUNK`/`GUIDE`/`NCX` `INDX` trees, trailing byte
//! sequences, and the KF8 MOBI header/`record0`).
//!
//! Port of `calibre.ebooks.mobi.writer8`, i.e. `cleanup.py`, `exth.py`,
//! `header.py`, `index.py`, `main.py`, `mobi.py`, `skeleton.py`,
//! `tbs.py`, `toc.py` (`__init__.py` is trivial license boilerplate with
//! no code, folded into this module doc rather than getting its own
//! file).
//!
//! This is the encoder half of the format
//! [`crate::mobi::mobi8::Mobi8Reader`] (issue #33) already decodes: the
//! `SKEL`/`DIV`/`GUIDE` `INDX` trees this module's [`index`]/[`skeleton`]
//! build are exactly what that reader parses back.

pub mod cleanup;
pub mod exth;
pub mod header;
pub mod index;
pub mod main;
pub mod mobi;
pub mod skeleton;
pub mod tbs;
pub mod toc;
