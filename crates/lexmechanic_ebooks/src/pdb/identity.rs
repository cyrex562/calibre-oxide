//! Which format a Palm database actually holds (#926).
//!
//! Port of `calibre.ebooks.pdb.__init__`'s `IDENTITY_TO_NAME` and
//! `FORMAT_READERS`. A `.pdb` is a **container**, not a format: the same
//! extension carries PalmDoc, eReader/PML, zTXT, Plucker, Haodoo and
//! embedded PDF, told apart only by the 8-byte type+creator pair in the
//! header. Dispatching on that is the difference between converting a
//! book and guessing.

use crate::pdb::header::PdbHeader;

/// A format a `.pdb` can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdbFormat {
    /// eReader, whose text is PML markup.
    EReader,
    /// Plain PalmDoc text.
    PalmDoc,
    /// A PDF carried inside a Palm database.
    Pdf,
    ZText,
    Plucker,
    Haodoo,
}

impl PdbFormat {
    /// The name upstream's `IDENTITY_TO_NAME` uses, for error messages a
    /// person can act on.
    pub fn name(self) -> &'static str {
        match self {
            PdbFormat::EReader => "eReader",
            PdbFormat::PalmDoc => "PalmDoc",
            PdbFormat::Pdf => "Adobe Reader",
            PdbFormat::ZText => "zTXT",
            PdbFormat::Plucker => "Plucker",
            PdbFormat::Haodoo => "Haodoo",
        }
    }
}

/// The header's 8-byte type+creator pair, as the ASCII string upstream
/// keys its tables by.
///
/// Lossy on purpose: these are supposed to be printable ASCII, and a file
/// with rubbish here needs to produce a diagnosable message rather than
/// an encoding error.
pub fn identity_of(header: &PdbHeader) -> String {
    identity_from(&header.type_id, &header.creator_id)
}

/// The identity from the two raw fields.
///
/// Exists separately from [`identity_of`] so this can be exercised
/// without fabricating a whole `PdbHeader` -- that struct is only ever
/// produced by parsing a real file, and giving it a `Default` would let
/// an all-zero header masquerade as a valid one.
pub fn identity_from(type_id: &[u8; 4], creator_id: &[u8; 4]) -> String {
    let mut ident = Vec::with_capacity(8);
    ident.extend_from_slice(type_id);
    ident.extend_from_slice(creator_id);
    String::from_utf8_lossy(&ident).into_owned()
}

/// Maps an identity to its format. `None` means no format upstream knows
/// about either.
pub fn format_for(identity: &str) -> Option<PdbFormat> {
    match identity {
        // Two eReader idents exist because the format was revised; both
        // are read by the same reader upstream.
        "PNPdPPrs" | "PNRdPPrs" => Some(PdbFormat::EReader),
        "TEXtREAd" => Some(PdbFormat::PalmDoc),
        ".pdfADBE" => Some(PdbFormat::Pdf),
        "zTXTGPlm" => Some(PdbFormat::ZText),
        "DataPlkr" => Some(PdbFormat::Plucker),
        "BOOKMTIT" | "BOOKMTIU" => Some(PdbFormat::Haodoo),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_the_type_and_creator_concatenated() {
        assert_eq!(identity_from(b"TEXt", b"REAd"), "TEXtREAd");
    }

    #[test]
    fn both_ereader_idents_map_to_the_same_format() {
        assert_eq!(format_for("PNPdPPrs"), Some(PdbFormat::EReader));
        assert_eq!(format_for("PNRdPPrs"), Some(PdbFormat::EReader));
    }

    #[test]
    fn every_upstream_identity_is_recognised() {
        // Transcribed from `IDENTITY_TO_NAME`. A missing entry here means
        // a real file that upstream reads and this does not.
        for (ident, expected) in [
            ("PNPdPPrs", PdbFormat::EReader),
            ("PNRdPPrs", PdbFormat::EReader),
            ("zTXTGPlm", PdbFormat::ZText),
            ("TEXtREAd", PdbFormat::PalmDoc),
            (".pdfADBE", PdbFormat::Pdf),
            ("DataPlkr", PdbFormat::Plucker),
            ("BOOKMTIT", PdbFormat::Haodoo),
            ("BOOKMTIU", PdbFormat::Haodoo),
        ] {
            assert_eq!(format_for(ident), Some(expected), "{ident} should be recognised");
        }
    }

    #[test]
    fn an_unknown_identity_is_none_rather_than_a_guess() {
        assert_eq!(format_for("XXXXYYYY"), None);
        assert_eq!(format_for(""), None);
    }

    /// Rubbish in the header must still produce something printable, so
    /// the error names what it found.
    #[test]
    fn a_non_ascii_identity_does_not_fail_to_render() {
        let identity = identity_from(&[0xff, 0xfe, 0x00, 0x41], b"REAd");
        assert!(identity.contains('A'), "{identity:?}");
        assert_eq!(format_for(&identity), None);
    }
}
