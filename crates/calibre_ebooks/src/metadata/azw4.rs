use crate::metadata::{pdf, MetaInformation};
use anyhow::{bail, Result};
use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    // AZW4 is a PDB container wrapping a PDF.
    // The PDF usually starts with %PDF and ends with %%EOF.
    // We can naively scan for the PDF content if unwrapping complex PDB records is difficult,
    // or we can implement proper PDB record reading if we want to be strict.
    // The legacy implementation (reader.py) unwraps by searching for %PDF...%%EOF.

    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer)?;

    let pdf_start_sig = b"%PDF";
    let pdf_end_sig = b"%%EOF";

    let start = buffer
        .windows(pdf_start_sig.len())
        .position(|window| window == pdf_start_sig);

    if let Some(start_idx) = start {
        // Find end. %%EOF might appear multiple times (incremental updates),
        // usually we want the last one or we just take the rest of the file
        // if we assume the PDF is the main payload.
        // Legacy code: m = re.search(br'%PDF.+%%EOF', raw_data, flags=re.DOTALL)
        // This regex matches greedily to the LAST %%EOF effectively if done right,
        // or first depending on flags. re.search usually finds first match of pattern?
        // Actually '.' matches anything including newline with DOTALL.
        // It finds the Longest match? Or first match?
        // Python re search finds the FIRST location where the pattern produces a match.
        // If the pattern is %PDF.+%%EOF, and there are multiple %%EOF,
        // + is greedy, so it will match until the LAST %%EOF.

        let end = buffer
            .windows(pdf_end_sig.len())
            .rposition(|window| window == pdf_end_sig);

        if let Some(mut end_idx) = end {
            end_idx += pdf_end_sig.len(); // Include the signature

            if end_idx > start_idx {
                let pdf_data = &buffer[start_idx..end_idx];
                let mut pdf_cursor = std::io::Cursor::new(pdf_data);
                return pdf::get_metadata(&mut pdf_cursor);
            }
        }
    }

    bail!("No embedded PDF found in AZW4 container")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_azw4_metadata_delegation() {
        eprintln!("DEBUG: Executing AZW4 test");
        // Mock AZW4 content: Some junk header + PDF Content + Some junk footer
        let pdf_content = b"%PDF-1.4 ... /Title (Mock AZW4 Title) ... %%EOF";
        let mut file_content = Vec::new();
        file_content.extend_from_slice(b"JUNK HEADER");
        file_content.extend_from_slice(pdf_content);
        file_content.extend_from_slice(b"JUNK FOOTER");

        // Note: For this to truly pass, pdf::get_metadata must be able to parse our mock PDF.
        // Since pdf::get_metadata uses lopdf, we need a valid minimal PDF structure.
        // Constructing a valid minimal PDF binary is non-trivial inline.
        // So we will verify the extraction logic by ensuring it CALLS pdf::get_metadata,
        // or we stub pdf::get_metadata if possible (not easy in Rust unit tests without mocking frameworks).

        // Alternatively, we trust pdf::get_metadata works (tested elsewhere)
        // and we just test the unwrapping logic if we split the function.

        // Let's create a "valid" minimal PDF for lopdf to read if possible.
        // Or we just checking if it returns error "No embedded PDF" vs "PDF Parse Error".

        let cursor = Cursor::new(file_content);
        let result = get_metadata(cursor);

        // It should probably fail with a PDF parsing error (since our PDF content is fake),
        // but NOT "No embedded PDF found".
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_ne!(err.to_string(), "No embedded PDF found in AZW4 container");
    }
}
