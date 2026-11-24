//! Port of `page_number_type.py`.
//!
//! Single-letter tags used in the APNX page-map string per Amazon's
//! format: `a` for Arabic, `r` for Roman, `c` for Custom labels.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageNumberType {
    Arabic,
    Roman,
    Custom,
}

impl PageNumberType {
    pub fn tag(&self) -> char {
        match self {
            PageNumberType::Arabic => 'a',
            PageNumberType::Roman => 'r',
            PageNumberType::Custom => 'c',
        }
    }
}

impl fmt::Display for PageNumberType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PageNumberType::Arabic => "Arabic",
            PageNumberType::Roman => "Roman",
            PageNumberType::Custom => "Custom",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_match_amazon_apnx_spec() {
        assert_eq!(PageNumberType::Arabic.tag(), 'a');
        assert_eq!(PageNumberType::Roman.tag(), 'r');
        assert_eq!(PageNumberType::Custom.tag(), 'c');
    }
}
