//! Port of `old_src/src/calibre/ebooks/oeb/polish/hyphenation.py`.
//!
//! Python delegates the actual per-word hyphenation to
//! `calibre.utils.hyphenation.hyphenate.add_soft_hyphens_to_html`, which
//! walks an `lxml` tree mutating `elem.text`/`child.tail` in place, and
//! looks up a per-locale dictionary via `calibre_extensions.hyphen`
//! (bundled `.dic` files for many languages, loaded through a small C
//! extension).
//!
//! This port reuses [`crate::hyphenate`] (issue-established: wraps the
//! `hyphenation` crate's Knuth-Liang algorithm, matching Python's own
//! algorithm) instead of re-deriving hyphenation from scratch. That
//! module only embeds the `en-US` pattern table (`embed_en-us` Cargo
//! feature) -- see its module docs -- so, exactly like Python's
//! `dictionary_for_locale` returning `None` for a locale with no bundled
//! dictionary file (a real, expected runtime outcome, not an error),
//! [`is_supported_locale`] returns `false` for anything other than an
//! `en`/`en-*` tag and this module silently leaves that text
//! unhyphenated. This is not a placeholder: it is the same "missing
//! dictionary -> no-op for that subtree's own text" behavior Python
//! exhibits, just with a smaller set of bundled dictionaries.
//!
//! # Tree-shape note
//!
//! Python's `add_to_tag` treats `elem.text` (leading text) and each
//! `child.tail` (trailing text after a child) as the two kinds of
//! "this element's own text" needing hyphenation, while recursing into
//! child *elements* regardless of whether a dictionary was found for the
//! current locale. [`crate::mobi::dom::Dom`] represents both of those
//! as ordinary sibling [`crate::mobi::dom::NodeKind::Text`] children of
//! `elem` (see that module's docs) -- so a single pass over `elem`'s
//! direct children, branching on `Text` vs `Element`, covers exactly the
//! same two cases Python's two separate code paths (`elem.text` /
//! `child.tail`) cover.

use crate::hyphenate::hyphenate_text;
use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::constants::OEB_DOCS;

use super::container::Container;

/// U+00AD SOFT HYPHEN, matching Python's default `hyphen_char`.
pub const SOFT_HYPHEN: char = '\u{ad}';

/// Port of `tags_not_to_hyphenate`.
pub const TAGS_NOT_TO_HYPHENATE: &[&str] = &[
    "video", "audio", "script", "code", "pre", "img", "br", "samp", "kbd", "var", "abbr",
    "acronym", "sub", "sup", "button", "option", "label", "textarea", "input", "math", "svg",
    "style", "title", "head",
];

/// Whether a `hyphenation` dictionary is bundled for `locale`. See the
/// module docs: only `en`/`en-*` is embedded in this workspace today.
pub fn is_supported_locale(locale: &str) -> bool {
    let l = locale.trim().to_lowercase();
    l == "en" || l.starts_with("en-") || l.starts_with("en_")
}

/// Port of `add_soft_hyphens_to_html`. `locale` is the book's default
/// language (Python's `container.mi.language`); an element's own
/// `lang`/`xml:lang` attribute overrides it for that element and its
/// descendants, matching `add_to_tag`'s `tl` computation.
pub fn add_soft_hyphens_to_html(dom: &mut Dom, locale: &str) {
    let Some(root_elem) = dom
        .node(dom.root)
        .children
        .iter()
        .copied()
        .find(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
    else {
        return;
    };
    let mut stack = vec![(root_elem, locale.to_string())];
    while let Some((elem, elem_locale)) = stack.pop() {
        add_to_tag(dom, elem, &elem_locale, &mut stack);
    }
}

fn add_to_tag(dom: &mut Dom, elem: NodeId, locale: &str, stack: &mut Vec<(NodeId, String)>) {
    let Some(tag) = dom.tag(elem).map(|t| t.to_string()) else {
        return;
    };
    if TAGS_NOT_TO_HYPHENATE.contains(&tag.as_str()) {
        return;
    }
    let tl = dom
        .node(elem)
        .attrs
        .get("lang")
        .or_else(|| dom.node(elem).attrs.get("xml:lang"))
        .cloned()
        .unwrap_or_else(|| locale.to_string());
    let has_dictionary = is_supported_locale(&tl);
    let children = dom.children(elem);
    for child in children {
        match &dom.node(child).kind {
            NodeKind::Text(text) => {
                if has_dictionary && !text.trim().is_empty() {
                    let hyphenated = hyphenate_text(text, &SOFT_HYPHEN.to_string());
                    if let NodeKind::Text(t) = &mut dom.node_mut(child).kind {
                        *t = hyphenated;
                    }
                }
            }
            NodeKind::Element(_) => stack.push((child, tl.clone())),
            _ => {}
        }
    }
}

/// Port of `remove_soft_hyphens_from_html`: strips every
/// [`SOFT_HYPHEN`] from every text node in the document (Python's
/// `root.iterdescendants()` walk over `.text`/`.tail`).
pub fn remove_soft_hyphens_from_html(dom: &mut Dom) {
    for node in &mut dom.nodes {
        if let NodeKind::Text(t) = &node.kind {
            if t.contains(SOFT_HYPHEN) {
                let replaced = t.replace(SOFT_HYPHEN, "");
                node.kind = NodeKind::Text(replaced);
            }
        }
    }
}

/// Port of `oeb.polish.hyphenation.add_soft_hyphens`: adds soft hyphens
/// to every `OEB_DOCS` content document in `container`. `report`, if
/// given, is called once with a final status message (Python's
/// `report(_('Soft hyphens added'))`).
pub fn add_soft_hyphens(
    container: &mut Container,
    locale: &str,
    mut report: Option<&mut dyn FnMut(&str)>,
) -> anyhow::Result<()> {
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml_mut(&name)?;
        add_soft_hyphens_to_html(dom, locale);
        container.dirty(&name);
    }
    if let Some(report) = report.as_mut() {
        report("Soft hyphens added");
    }
    Ok(())
}

/// Port of `oeb.polish.hyphenation.remove_soft_hyphens`.
pub fn remove_soft_hyphens(
    container: &mut Container,
    mut report: Option<&mut dyn FnMut(&str)>,
) -> anyhow::Result<()> {
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml_mut(&name)?;
        remove_soft_hyphens_from_html(dom);
        container.dirty(&name);
    }
    if let Some(report) = report.as_mut() {
        report("Soft hyphens removed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_locale_matches_en_variants() {
        assert!(is_supported_locale("en"));
        assert!(is_supported_locale("en-US"));
        assert!(is_supported_locale("en_GB"));
        assert!(!is_supported_locale("fr"));
        assert!(!is_supported_locale(""));
    }

    #[test]
    fn adds_soft_hyphens_to_english_text() {
        let mut dom = Dom::parse("<html><body><p>hyphenation testing</p></body></html>");
        add_soft_hyphens_to_html(&mut dom, "en");
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(out.contains(&SOFT_HYPHEN.to_string()));
        // Round trip: stripping the markers gives back the original text.
        assert!(out.replace(SOFT_HYPHEN, "").contains("hyphenation testing"));
    }

    #[test]
    fn skips_unsupported_locale() {
        let mut dom =
            Dom::parse("<html lang=\"fr\"><body><p>hyphenation testing</p></body></html>");
        add_soft_hyphens_to_html(&mut dom, "fr");
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(!out.contains(SOFT_HYPHEN));
    }

    #[test]
    fn does_not_recurse_into_excluded_tags() {
        let mut dom = Dom::parse("<html><body><pre>hyphenation testing</pre></body></html>");
        add_soft_hyphens_to_html(&mut dom, "en");
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(!out.contains(SOFT_HYPHEN));
    }

    #[test]
    fn removes_soft_hyphens() {
        let mut dom = Dom::parse("<html><body><p>hyphenation testing</p></body></html>");
        add_soft_hyphens_to_html(&mut dom, "en");
        remove_soft_hyphens_from_html(&mut dom);
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(!out.contains(SOFT_HYPHEN));
        assert!(out.contains("hyphenation testing"));
    }
}
