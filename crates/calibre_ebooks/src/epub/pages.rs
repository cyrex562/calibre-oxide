//! EPUB 2 page maps.
//!
//! Port of `old_src/src/calibre/ebooks/epub/pages.py`.
//!
//! A page map ties positions in the reflowed text back to the page
//! numbers of a print edition, so a reader can show "page 42 of the
//! paperback". The names come either from a counter or from text the
//! book itself carries at each break — `<a id="page_42">` in a Project
//! Gutenberg scan, say — which is what [`filter_name`] cleans up.
//!
//! # Two deviations, both forced
//!
//! **`add_page_map` cannot be ported as written.** Its last two lines
//! are:
//!
//! ```python
//! writer = None  # DirWriter(version='2.0', page_map=True)
//! writer.dump(oeb, opfpath)
//! ```
//!
//! so any call raises `AttributeError` before returning. Nothing in
//! calibre calls it — the function has been unreachable since the
//! writer was commented out. This port keeps the part that works
//! (choosing names, assigning ids, building the page list) and returns
//! the assignments instead of crashing.
//!
//! **Element selection is the caller's.** The Python takes two XPath
//! expressions from the command line, one selecting the break elements
//! and one pulling name text out of each. Evaluating arbitrary XPath
//! needs an engine this crate does not have, so the caller does the
//! selecting and hands over [`PageMarker`]s. Everything downstream of
//! that — the id generation, the `href#id` joining, the name filtering
//! — is ported.

use std::sync::OnceLock;

use regex::Regex;

use crate::oeb::book::OEBBook;

fn page_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)page").expect("valid regex"))
}

fn roman_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^[ivxlcdm]+$").expect("valid regex"))
}

/// Reduce the text found at a page break to the page name itself.
///
/// Port of the Python `filter_name`: drop the word "page" wherever it
/// appears, then, if any remaining word is a number or a roman
/// numeral, use just that word.
pub fn filter_name(name: &str) -> String {
    let stripped = page_re().replace_all(name.trim(), "").into_owned();
    for word in stripped.split_whitespace() {
        if !word.is_empty()
            && (word.chars().all(|c| c.is_ascii_digit()) || roman_re().is_match(word))
        {
            return word.to_string();
        }
    }
    stripped
}

/// Where the names of pages come from.
///
/// Port of the Python `build_name_for`, which returns either a counter
/// or an XPath-driven namer depending on whether `--page-names` was
/// given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageNamer {
    /// Number the pages 1, 2, 3, ... in spine order.
    Sequential { next: u32 },
    /// Name each page from the text the caller extracted for it.
    FromText,
}

impl PageNamer {
    /// A counter starting at 1, as the Python's `count(1)` does.
    pub fn sequential() -> Self {
        Self::Sequential { next: 1 }
    }

    /// The name for one page break.
    pub fn name_for(&mut self, values: &[String]) -> String {
        match self {
            Self::Sequential { next } => {
                let name = next.to_string();
                *next += 1;
                name
            }
            Self::FromText => {
                if values.is_empty() {
                    return String::new();
                }
                filter_name(&values.join(" "))
            }
        }
    }
}

/// One page break the caller found in a spine item.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageMarker {
    /// The element's existing `id`, if it has one. When absent, a
    /// generated id is assigned and reported back.
    pub id: Option<String>,
    /// The strings the name expression selected from this element.
    /// Ignored by [`PageNamer::Sequential`].
    pub values: Vec<String>,
}

/// A page break after processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedPage {
    /// The spine item the break is in.
    pub item_href: String,
    /// The element's id — either the one it had or a generated one.
    pub id: String,
    /// Whether the id was generated, meaning the caller must write it
    /// back into the document for the map's links to resolve.
    pub id_generated: bool,
    /// The page name recorded in the map.
    pub name: String,
    /// The `item#id` target recorded in the map.
    pub href: String,
}

/// Add a page map to `oeb` from page breaks the caller has located.
///
/// `items` pairs each spine item's href with the breaks found in it, in
/// document order. Returns the assignments, including any generated
/// ids, which the caller writes back into its own document tree.
///
/// Port of the Python `add_page_map`, minus its unreachable final two
/// lines — see the module docs.
pub fn add_page_map(
    oeb: &mut OEBBook,
    items: &[(String, Vec<PageMarker>)],
    namer: &mut PageNamer,
) -> Vec<AssignedPage> {
    let mut assigned = Vec::new();
    let mut next_id = 1u32;
    for (item_href, markers) in items {
        for marker in markers {
            let name = namer.name_for(&marker.values);
            let (id, id_generated) = match marker.id.as_deref().filter(|i| !i.is_empty()) {
                Some(id) => (id.to_string(), false),
                None => {
                    let id = format!("calibre-page-{next_id}");
                    next_id += 1;
                    (id, true)
                }
            };
            let href = format!("{item_href}#{id}");
            oeb.pages.add(&name, &href, "normal");
            assigned.push(AssignedPage {
                item_href: item_href.clone(),
                id,
                id_generated,
                name,
                href,
            });
        }
    }
    assigned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn book() -> OEBBook {
        OEBBook::new(Box::new(DirContainer::new(std::path::Path::new("."))))
    }

    fn marker(id: Option<&str>, values: &[&str]) -> PageMarker {
        PageMarker {
            id: id.map(str::to_string),
            values: values.iter().map(|v| v.to_string()).collect(),
        }
    }

    #[test]
    fn filter_name_extracts_the_number_from_a_page_label() {
        assert_eq!(filter_name("Page 42"), "42");
        assert_eq!(filter_name("page 42"), "42");
        assert_eq!(filter_name("  PAGE   42  "), "42");
        assert_eq!(filter_name("42"), "42");
    }

    #[test]
    fn filter_name_understands_roman_numerals() {
        assert_eq!(filter_name("Page xiv"), "xiv");
        assert_eq!(filter_name("Page XIV"), "XIV");
        // A roman numeral is only recognised as a whole word — and
        // when nothing matches, the text is returned as the `page`
        // removal left it, leading space included. calibre strips only
        // before the substitution, never after.
        assert_eq!(filter_name("Page xiv-a"), " xiv-a");
    }

    #[test]
    fn filter_name_keeps_text_that_is_not_a_number() {
        // Nothing numeric, so the de-"page"d text is used as it is.
        assert_eq!(filter_name("Front matter"), "Front matter");
        assert_eq!(filter_name(""), "");
        // "page" is removed wherever it occurs, including inside a
        // word — the Python's regex is not word-bounded.
        assert_eq!(filter_name("homepage"), "home");
    }

    #[test]
    fn filter_name_picks_the_first_numeric_word() {
        assert_eq!(filter_name("Page 7 of 350"), "7");
        // "2," is not a bare number — the comma disqualifies it — so
        // the first word that does qualify is 15.
        assert_eq!(filter_name("Chapter 2, page 15"), "15");
    }

    #[test]
    fn a_sequential_namer_counts_from_one() {
        let mut namer = PageNamer::sequential();
        let names: Vec<String> = (0..3).map(|_| namer.name_for(&[])).collect();
        assert_eq!(names, vec!["1", "2", "3"]);
    }

    #[test]
    fn a_text_namer_joins_and_filters_the_selected_values() {
        let mut namer = PageNamer::FromText;
        assert_eq!(
            namer.name_for(&["Page".to_string(), "42".to_string()]),
            "42"
        );
        // No values selected means no name, as in the Python.
        assert_eq!(namer.name_for(&[]), "");
    }

    #[test]
    fn existing_ids_are_reused_and_missing_ones_generated() {
        let mut oeb = book();
        let mut namer = PageNamer::sequential();
        let items = vec![(
            "text/chapter1.html".to_string(),
            vec![
                marker(Some("pg1"), &[]),
                marker(None, &[]),
                marker(Some(""), &[]),
            ],
        )];
        let assigned = add_page_map(&mut oeb, &items, &mut namer);

        assert_eq!(assigned.len(), 3);
        assert_eq!(assigned[0].id, "pg1");
        assert!(!assigned[0].id_generated);
        assert_eq!(assigned[1].id, "calibre-page-1");
        assert!(assigned[1].id_generated);
        // An empty id counts as absent.
        assert_eq!(assigned[2].id, "calibre-page-2");
        assert_eq!(assigned[0].href, "text/chapter1.html#pg1");
    }

    #[test]
    fn the_page_list_records_every_break_in_spine_order() {
        let mut oeb = book();
        let mut namer = PageNamer::sequential();
        let items = vec![
            (
                "a.html".to_string(),
                vec![marker(Some("p1"), &[]), marker(Some("p2"), &[])],
            ),
            ("b.html".to_string(), vec![marker(Some("p3"), &[])]),
        ];
        add_page_map(&mut oeb, &items, &mut namer);

        let pages: Vec<(&str, &str)> = oeb
            .pages
            .pages
            .iter()
            .map(|p| (p.name.as_str(), p.href.as_str()))
            .collect();
        assert_eq!(
            pages,
            vec![("1", "a.html#p1"), ("2", "a.html#p2"), ("3", "b.html#p3"),]
        );
        assert!(oeb.pages.pages.iter().all(|p| p.type_ == "normal"));
    }

    #[test]
    fn generated_ids_do_not_restart_between_spine_items() {
        let mut oeb = book();
        let mut namer = PageNamer::sequential();
        let items = vec![
            ("a.html".to_string(), vec![marker(None, &[])]),
            ("b.html".to_string(), vec![marker(None, &[])]),
        ];
        let assigned = add_page_map(&mut oeb, &items, &mut namer);
        assert_eq!(assigned[0].id, "calibre-page-1");
        assert_eq!(assigned[1].id, "calibre-page-2");
    }

    #[test]
    fn names_can_come_from_the_documents_own_text() {
        let mut oeb = book();
        let mut namer = PageNamer::FromText;
        let items = vec![(
            "scan.html".to_string(),
            vec![
                marker(Some("a"), &["Page", "iv"]),
                marker(Some("b"), &["Page 12"]),
                marker(Some("c"), &[]),
            ],
        )];
        let assigned = add_page_map(&mut oeb, &items, &mut namer);
        let names: Vec<&str> = assigned.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["iv", "12", ""]);
    }

    #[test]
    fn a_book_with_no_breaks_gets_an_empty_page_map() {
        let mut oeb = book();
        let assigned = add_page_map(&mut oeb, &[], &mut PageNamer::sequential());
        assert!(assigned.is_empty());
        assert!(oeb.pages.pages.is_empty());
    }
}
