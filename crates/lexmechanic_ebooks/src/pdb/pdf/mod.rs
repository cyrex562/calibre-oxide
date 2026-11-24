//! Port of `calibre.ebooks.pdb.pdf` -- a PDF file wrapped whole inside a
//! PDB container (each PDB section is a raw slice of the PDF bytes,
//! concatenated back together on read).

pub mod reader;
