pub struct DOCXNamespaces;

impl DOCXNamespaces {
    pub const DOC_PROPS: &'static str =
        "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
    pub const M: &'static str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
    pub const R: &'static str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    pub const W: &'static str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    pub const CP: &'static str =
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
    pub const DC: &'static str = "http://purl.org/dc/elements/1.1/";
    pub const DCTERMS: &'static str = "http://purl.org/dc/terms/";

    // Relationship Types
    pub const DOCUMENT: &'static str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const CORE_PROPS: &'static str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const EXTENDED_PROPS: &'static str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
}
