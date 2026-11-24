use crate::metadata::MetaInformation;
use crate::opf::parse_opf as parse_opf_xml;
use anyhow::{Context, Result};

pub struct OpfVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

pub fn parse_opf_version(raw: &str) -> OpfVersion {
    let parts: Vec<&str> = raw.split('.').collect();
    let major = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(2);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    OpfVersion {
        major,
        minor,
        patch,
    }
}

pub fn parse_opf(raw: &str) -> Result<MetaInformation> {
    parse_opf_xml(raw).context("Failed to parse OPF")
}

// Additional helpers can be added as needed.
pub fn clean_xml(raw: &str) -> String {
    // Basic cleanup; advanced entities handled by roxmltree usually?
    // Actually roxmltree handles XML strictness.
    // This is a placeholder for calibre's `clean_xml_chars` if needed.
    raw.replace('\0', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        let v = parse_opf_version("2.0");
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 0);

        let v = parse_opf_version("3.1.2");
        assert_eq!(v.major, 3);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 2);
    }
}
