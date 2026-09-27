//! `.pdb` input dispatches on the container's identity (#926).
//!
//! The defect this covers was silent: every `.pdb` produced an HTML page
//! listing "PDB Records" with hex previews and no error, so a user could
//! not tell a failed conversion from a book full of garbage.

use calibre_ebooks::input::pdb_input::PDBInput;
use calibre_ebooks::pdb::writer::PdbWriter;
use std::fs::File;
use std::io::{BufWriter, Write};
use tempfile::tempdir;

/// Writes a Palm database with the given type+creator identity.
///
/// Hand-rolled rather than via `PdbWriter`, which always stamps the
/// PalmDoc identity -- the point here is to vary it.
fn write_pdb_with_identity(path: &std::path::Path, identity: &[u8; 8], records: &[&[u8]]) {
    let mut buffer = Vec::new();
    let mut name = b"Test Book".to_vec();
    name.resize(32, 0);
    buffer.extend_from_slice(&name);
    buffer.extend_from_slice(&[0u8; 2]); // attributes
    buffer.extend_from_slice(&[0u8; 2]); // version
    buffer.extend_from_slice(&[0u8; 4 * 6]); // dates, ids
    buffer.extend_from_slice(identity);
    buffer.extend_from_slice(&[0u8; 4]); // unique id seed
    buffer.extend_from_slice(&[0u8; 4]); // next record list id
    buffer.extend_from_slice(&(records.len() as u16).to_be_bytes());

    let mut offset = buffer.len() + 8 * records.len() + 2;
    for (i, record) in records.iter().enumerate() {
        buffer.extend_from_slice(&(offset as u32).to_be_bytes());
        buffer.push(0);
        buffer.extend_from_slice(&((2 * i) as u32).to_be_bytes()[1..4]);
        offset += record.len();
    }
    buffer.extend_from_slice(&[0, 0]);
    for record in records {
        buffer.extend_from_slice(record);
    }
    std::fs::write(path, &buffer).unwrap();
}


/// `OEBBook` has no `Debug`, so `expect_err` will not compile here.
fn expect_refusal(result: anyhow::Result<calibre_ebooks::oeb::book::OEBBook>, what: &str) -> String {
    match result {
        Ok(_) => panic!("{what}"),
        Err(e) => format!("{e:#}"),
    }
}

/// A real eReader PML book converts, which is the case that was
/// unreachable: the working converter existed but nothing routed to it.
#[test]
fn an_ereader_pdb_converts_through_the_pml_reader() {
    let dir = tempdir().unwrap();
    let pdb_path = dir.path().join("book.pdb");
    let writer = BufWriter::new(File::create(&pdb_path).unwrap());
    let mut writer = writer;
    PdbWriter::new().write("PML Test", b"\\x1bp\\x1bpHello there.", &mut writer).unwrap();
    writer.flush().unwrap();
    drop(writer);

    // `PdbWriter` stamps the PalmDoc identity, so rewrite it in place to
    // the eReader one -- the identity is at offset 60.
    let mut bytes = std::fs::read(&pdb_path).unwrap();
    bytes[60..68].copy_from_slice(b"PNRdPPrs");
    std::fs::write(&pdb_path, &bytes).unwrap();

    let out = dir.path().join("out");
    let book = PDBInput::new().convert(&pdb_path, &out).expect("an eReader pdb should convert");
    assert!(!book.spine.items.is_empty(), "the converted book should have a spine");
}

/// The heart of #926: a format this cannot read must say so.
#[test]
fn a_ztxt_pdb_is_refused_by_name_rather_than_dumped() {
    let dir = tempdir().unwrap();
    let pdb_path = dir.path().join("book.pdb");
    write_pdb_with_identity(&pdb_path, b"zTXTGPlm", &[b"whatever"]);

    let out = dir.path().join("out");
    let message = expect_refusal(PDBInput::new().convert(&pdb_path, &out), "zTXT is not supported and must not pretend otherwise");
    assert!(message.contains("zTXT"), "the error should name the format: {message}");

    // The old behaviour's signature: a hex-preview page and record dumps.
    assert!(!out.join("index.html").exists(), "no fabricated index page should be produced");
    assert!(!out.join("record_0.bin").exists(), "records should not be dumped as .bin files");
}

#[test]
fn plucker_and_haodoo_are_also_refused_by_name() {
    for (identity, name) in [(b"DataPlkr", "Plucker"), (b"BOOKMTIT", "Haodoo")] {
        let dir = tempdir().unwrap();
        let pdb_path = dir.path().join("book.pdb");
        write_pdb_with_identity(&pdb_path, identity, &[b"whatever"]);

        let message = expect_refusal(PDBInput::new().convert(&pdb_path, &dir.path().join("out")), "should be refused");
        assert!(message.contains(name), "the error should name {name}: {message}");
    }
}

/// An unrecognised identity is a different message from a recognised but
/// unimplemented one -- "your file may be corrupt" rather than "this
/// format is not done yet".
#[test]
fn an_unrecognised_identity_says_it_matches_no_known_format() {
    let dir = tempdir().unwrap();
    let pdb_path = dir.path().join("book.pdb");
    write_pdb_with_identity(&pdb_path, b"XXXXYYYY", &[b"whatever"]);

    let message = expect_refusal(PDBInput::new().convert(&pdb_path, &dir.path().join("out")), "should be refused");
    assert!(message.contains("no known format"), "{message}");
    assert!(message.contains("XXXXYYYY"), "the error should quote what it found: {message}");
}
