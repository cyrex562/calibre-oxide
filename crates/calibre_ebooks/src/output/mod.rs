use crate::metadata::meta::MetaInformation;
use crate::oeb::book::OEBBook;

pub mod docx_output;
pub mod epub_output;
pub mod fb2_output;
pub mod html_output;
pub mod htmlz_output;
pub mod lit_output;
pub mod lrf_output;
pub mod mobi_output;
pub mod odt_output;
pub mod oeb_output;
pub mod pdb_output;
pub mod pdf_output;
pub mod pml_output;
pub mod rb_output;
pub mod rtf_output;
pub mod snb_output;
pub mod tcr_output;
pub mod txt_output;

/// A book's metadata as a [`crate::metadata::meta::MetaInformation`].
///
/// Shared by the output plugins that write a metadata part -- DOCX's
/// `docProps/core.xml`, ODT's `meta.xml`. Kept here rather than copied per
/// plugin so the term lookup cannot drift between them.
///
/// The inverse of `oeb::transforms::metadata::meta_info_to_oeb_metadata`,
/// narrowed to the six fields the DOCX property parts actually read
/// (`core_properties` uses title/authors/languages/tags/comments,
/// `app_properties` uses publisher). Adding more would be inventing
/// requirements this output does not have.
///
/// Terms are looked up both bare and `dc:`-prefixed, because both forms
/// occur: producers in this crate add bare ones, while a book read from an
/// OPF carries them namespaced.
pub(crate) fn metadata_of(book: &OEBBook) -> MetaInformation {
    let first = |terms: [&str; 2]| -> Option<String> { terms.iter().find_map(|t| book.metadata.get(t).first().map(|i| i.value.clone())) };
    let all = |terms: [&str; 2]| -> Vec<String> {
        for term in terms {
            let values: Vec<String> = book.metadata.get(term).iter().map(|i| i.value.clone()).collect();
            if !values.is_empty() {
                return values;
            }
        }
        Vec::new()
    };

    let mut mi = MetaInformation::default();
    if let Some(title) = first(["title", "dc:title"]).filter(|t| !t.trim().is_empty()) {
        mi.title = title;
    }
    let authors = all(["creator", "dc:creator"]);
    if !authors.is_empty() {
        mi.authors = authors;
    }
    let languages = all(["language", "dc:language"]);
    if !languages.is_empty() {
        mi.languages = languages;
    }
    mi.tags = all(["subject", "dc:subject"]);
    mi.publisher = first(["publisher", "dc:publisher"]).filter(|p| !p.trim().is_empty());
    mi.comments = first(["description", "dc:description"]).filter(|c| !c.trim().is_empty());
    mi
}
