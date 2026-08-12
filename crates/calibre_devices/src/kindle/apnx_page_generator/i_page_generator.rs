//! Port of `i_page_generator.py`.
//!
//! The abstract-base-class + free helper functions. Concrete
//! generator implementations (Fast/Accurate/Exact/Pagebreak) live
//! under `generators/` — separate issue.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

use super::pages::Pages;

/// The generator trait. Corresponds to Python's `IPageGenerator`
/// ABC. `generate` implements the try-then-fallback flow — a
/// generator that returns zero pages, OR raises, defers to its
/// fallback. The one exception: `FastPageGenerator` propagates errors
/// unchanged (there's nothing to fall back to; a bad fast-guess is
/// still a valid last-resort estimate).
pub trait IPageGenerator {
    /// Human-readable name — used to detect the FastPageGenerator
    /// special case in `generate`.
    fn name(&self) -> &str;

    /// The primary generation strategy.
    fn generate_primary(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages>;

    /// Fallback used when `generate_primary` returns zero pages OR
    /// fails (unless `self.name() == "FastPageGenerator"`).
    fn generate_fallback(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages>;

    fn generate(&self, mobi_file_path: &Path, real_count: Option<u32>) -> Result<Pages> {
        match self.generate_primary(mobi_file_path, real_count) {
            Ok(pages) => {
                if pages.number_of_pages() > 0 {
                    Ok(pages)
                } else {
                    self.generate_fallback(mobi_file_path, real_count)
                }
            }
            Err(e) => {
                if self.name() == "FastPageGenerator" {
                    Err(e)
                } else {
                    self.generate_fallback(mobi_file_path, real_count)
                }
            }
        }
    }
}

/// Port of `mobi_html_length` — reads the first PDB section of a MOBI
/// file and returns the `text_length` field (bytes 4..8 as big-endian
/// u32).
///
/// Uses direct file I/O rather than pulling in the full mobi reader —
/// we only need the first 8 bytes of section 0.
pub fn mobi_html_length(mobi_file_path: &Path) -> Result<u32> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut f = File::open(mobi_file_path)
        .with_context(|| format!("open {:?}", mobi_file_path))?;

    // Palm DB header layout: 78 bytes fixed header, then a 2-byte
    // number-of-records, then N × 8-byte record info entries. The
    // first record's data offset lives at file offset 78 + 2 = 80,
    // in the first 4 bytes of the record-info entry (big-endian u32).
    f.seek(SeekFrom::Start(78))?;
    let mut n_records_buf = [0u8; 2];
    f.read_exact(&mut n_records_buf).context("read record count")?;
    let n_records = u16::from_be_bytes(n_records_buf);
    if n_records == 0 {
        return Err(anyhow!("MOBI file has zero PDB records"));
    }

    // Record 0's offset.
    let mut rec0_offset_buf = [0u8; 4];
    f.read_exact(&mut rec0_offset_buf).context("read record 0 offset")?;
    let rec0_offset = u32::from_be_bytes(rec0_offset_buf);

    // Section 0 layout: bytes 0..2 = compression, 2..4 = unused,
    // 4..8 = text length. We need 4..8.
    f.seek(SeekFrom::Start(rec0_offset as u64 + 4))?;
    let mut text_len_buf = [0u8; 4];
    f.read_exact(&mut text_len_buf).context("read text length")?;
    Ok(u32::from_be_bytes(text_len_buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct StubGenerator {
        name: &'static str,
        primary: Result<Pages>,
        fallback_pages: Pages,
    }

    impl IPageGenerator for StubGenerator {
        fn name(&self) -> &str {
            self.name
        }
        fn generate_primary(&self, _p: &Path, _rc: Option<u32>) -> Result<Pages> {
            match &self.primary {
                Ok(p) => Ok(p.clone()),
                Err(e) => Err(anyhow!("{}", e)),
            }
        }
        fn generate_fallback(&self, _p: &Path, _rc: Option<u32>) -> Result<Pages> {
            Ok(self.fallback_pages.clone())
        }
    }

    fn path() -> PathBuf {
        PathBuf::from("unused.mobi")
    }

    #[test]
    fn primary_success_with_pages_wins() {
        let g = StubGenerator {
            name: "AccuratePageGenerator",
            primary: Ok(Pages::from_arabic_locations(vec![100, 200])),
            fallback_pages: Pages::from_arabic_locations(vec![999]),
        };
        let out = g.generate(&path(), None).unwrap();
        assert_eq!(out.number_of_pages(), 2);
        assert_eq!(out.page_locations(), vec![100, 200]);
    }

    #[test]
    fn primary_zero_pages_triggers_fallback() {
        let g = StubGenerator {
            name: "AccuratePageGenerator",
            primary: Ok(Pages::new()),
            fallback_pages: Pages::from_arabic_locations(vec![50, 150]),
        };
        let out = g.generate(&path(), None).unwrap();
        assert_eq!(out.page_locations(), vec![50, 150]);
    }

    #[test]
    fn primary_error_falls_back_for_non_fast_generator() {
        let g = StubGenerator {
            name: "AccuratePageGenerator",
            primary: Err(anyhow!("boom")),
            fallback_pages: Pages::from_arabic_locations(vec![7]),
        };
        let out = g.generate(&path(), None).unwrap();
        assert_eq!(out.page_locations(), vec![7]);
    }

    #[test]
    fn fast_generator_propagates_error_instead_of_falling_back() {
        let g = StubGenerator {
            name: "FastPageGenerator",
            primary: Err(anyhow!("boom")),
            fallback_pages: Pages::from_arabic_locations(vec![7]),
        };
        let res = g.generate(&path(), None);
        assert!(res.is_err(), "FastPageGenerator must not use fallback");
        assert_eq!(res.unwrap_err().to_string(), "boom");
    }

    #[test]
    fn mobi_html_length_reads_synthetic_pdb() {
        use std::io::Write;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut w = std::fs::File::create(tmp.path()).unwrap();

        // 78-byte header (arbitrary bytes).
        w.write_all(&[0u8; 78]).unwrap();
        // Two records, so n_records = 2.
        w.write_all(&2u16.to_be_bytes()).unwrap();
        // Record 0 info: offset=100, uid/attr filler.
        w.write_all(&100u32.to_be_bytes()).unwrap();
        w.write_all(&[0u8; 4]).unwrap();
        // Record 1 info: offset=200, filler.
        w.write_all(&200u32.to_be_bytes()).unwrap();
        w.write_all(&[0u8; 4]).unwrap();
        // Pad to offset 100.
        let pos = w.metadata().unwrap().len();
        let pad = 100u64.saturating_sub(pos);
        w.write_all(&vec![0u8; pad as usize]).unwrap();
        // Record 0: compression(2) unused(2) text_length(4)=12345.
        w.write_all(&[0u8; 4]).unwrap();
        w.write_all(&12345u32.to_be_bytes()).unwrap();
        w.flush().unwrap();
        drop(w);

        let got = mobi_html_length(tmp.path()).unwrap();
        assert_eq!(got, 12345);
    }
}
