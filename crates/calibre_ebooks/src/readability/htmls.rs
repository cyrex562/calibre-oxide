//! Port of `old_src/src/calibre/ebooks/readability/htmls.py`.

use indexmap::IndexSet;
use regex::Regex;

use crate::chardet::xml_to_unicode;
use crate::mobi::dom::{Dom, NodeId};

use super::cleaners::{clean_attributes, normalize_spaces};
use super::direct_text;

/// Port of `build_doc`: decode `page` (using the same
/// `xml_to_unicode(page, strip_encoding_pats=True)` call `htmls.py`
/// makes) and parse it as an HTML5 document.
pub fn build_doc(page: &[u8]) -> Dom {
    let (page_unicode, _encoding) = xml_to_unicode(page, true, false);
    Dom::parse(&page_unicode)
}

/// Port of `js_re`. Dead code upstream: nothing in `old_src` -- not even
/// `readability.py`'s `main()` -- calls `js_re`. It's ported for
/// completeness since it's part of `htmls.py`, but note two things
/// about faithfully replicating it:
///
/// - The Python's own argument order is unusual: it calls
///   `re.compile(pattern, flags).sub(src, repl.replace('$', '\\'))`,
///   i.e. it passes `src` as the *replacement* and the `$`-to-`\`-translated
///   `repl` as the *string being searched*. That looks like the
///   two arguments were swapped by mistake (a typical call would search
///   `src` and substitute in `repl`), but since there is no caller to
///   observe the difference, it's preserved here rather than "corrected"
///   -- `search`/`replacement` below play the same (swapped) roles.
/// - `flags` in Python is `re`'s integer bitmask; the closest faithful
///   stand-in without inventing a bitmask type nothing else in this
///   crate uses is a single `case_insensitive` bool (the only flag any
///   hypothetical caller would plausibly want), documented as a
///   deliberate narrowing since there is no real call site to get this
///   wrong for.
pub fn js_re(
    search: &str,
    pattern: &str,
    case_insensitive: bool,
    replacement: &str,
) -> Result<String, regex::Error> {
    let re = if case_insensitive {
        Regex::new(&format!("(?i){pattern}"))?
    } else {
        Regex::new(pattern)?
    };
    // Python's `repl.replace('$', '\\')` rewrites JS-style `$1` group
    // references into Python's `\1` syntax for `re.sub`'s replacement
    // string. Rust's `regex` crate replacement strings already use `$1`
    // natively, so no such rewrite is needed here for equivalent
    // *group-substitution* behavior -- this is just an argument-order
    // note, not a behavior implemented, since (again) nothing calls this.
    Ok(re.replace_all(search, replacement).into_owned())
}

/// Port of `normalize_entities`: replace a fixed set of dash/space/quote
/// variants (Unicode chars and a couple of literal HTML entities) with
/// their ASCII equivalents.
pub fn normalize_entities(cur_title: &str) -> String {
    const REPLACEMENTS: &[(&str, &str)] = &[
        ("\u{2014}", "-"),
        ("\u{2013}", "-"),
        ("&mdash;", "-"),
        ("&ndash;", "-"),
        ("\u{00a0}", " "),
        ("\u{00ab}", "\""),
        ("\u{00bb}", "\""),
        ("&quot;", "\""),
    ];
    let mut out = cur_title.to_string();
    for (from, to) in REPLACEMENTS {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// Port of `norm_title`.
pub fn norm_title(title: &str) -> String {
    normalize_entities(&normalize_spaces(title))
}

/// Port of `get_title`. `doc` is the whole parsed document (what
/// `Document::_html` produces), matching every real call site.
pub fn get_title(doc: &Dom) -> String {
    let title = doc
        .find_first_tag_global("title")
        .and_then(|id| direct_text(doc, id))
        .filter(|t| !t.is_empty());
    match title {
        Some(t) => norm_title(&t),
        None => "[no-title]".to_string(),
    }
}

/// Port of `add_match`. `collection` uses [`IndexSet`] rather than a
/// plain `HashSet` so [`shorten_title`]'s tie-break (see its doc
/// comment) has a stable, reproducible order to work with -- the
/// Python's `set()` here has no defined iteration order at all.
pub(crate) fn add_match(collection: &mut IndexSet<String>, text: &str, orig: &str) {
    let text = norm_title(text);
    if text.split_whitespace().count() >= 2 && text.chars().count() >= 15 {
        let text_stripped = text.replace('"', "");
        let orig_stripped = orig.replace('"', "");
        if orig_stripped.contains(&text_stripped) {
            collection.insert(text);
        }
    }
}

fn elements_with_id(dom: &Dom, id_value: &str) -> Vec<NodeId> {
    dom.preorder_elements(dom.root)
        .into_iter()
        .filter(|&n| dom.node(n).attrs.get("id").map(|s| s.as_str()) == Some(id_value))
        .collect()
}

fn elements_with_class_token(dom: &Dom, class_value: &str) -> Vec<NodeId> {
    dom.preorder_elements(dom.root)
        .into_iter()
        .filter(|&n| {
            dom.node(n)
                .attrs
                .get("class")
                .is_some_and(|c| c.split_whitespace().any(|w| w == class_value))
        })
        .collect()
}

fn collect_candidate(dom: &Dom, e: NodeId, orig: &str, candidates: &mut IndexSet<String>) {
    if let Some(t) = direct_text(dom, e) {
        if !t.is_empty() {
            add_match(candidates, &t, orig);
        }
    }
    let tc = dom.text_content(e);
    if !tc.is_empty() {
        add_match(candidates, &tc, orig);
    }
}

/// Port of `shorten_title`: find the shortest plausible "real" title by
/// scanning `h1`/`h2`/`h3` headings and ten specific id/class patterns
/// for text that's a substring of the `<title>` text, then falling back
/// to splitting the `<title>` text on a handful of common
/// site-name/separator delimiters (` | `, ` - `, ` :: `, ` / `, `: `).
pub fn shorten_title(doc: &Dom) -> String {
    let Some(title_id) = doc.find_first_tag_global("title") else {
        return String::new();
    };
    let Some(title_text) = direct_text(doc, title_id).filter(|t| !t.is_empty()) else {
        return String::new();
    };

    let orig = norm_title(&title_text);
    let mut title = orig.clone();

    let mut candidates: IndexSet<String> = IndexSet::new();

    for tag in ["h1", "h2", "h3"] {
        for e in doc.find_all_tag_global(tag) {
            collect_candidate(doc, e, &orig, &mut candidates);
        }
    }

    // The ten `descendant-or-self::*[...]` XPath patterns from the
    // Python, translated to direct id/class-token lookups (see the
    // module doc comment on why `Dom` doesn't need real XPath support
    // for this -- each pattern is either an exact `@id` match or a
    // whitespace-tokenized `@class` match).
    for id_value in ["title", "head", "heading"] {
        for e in elements_with_id(doc, id_value) {
            collect_candidate(doc, e, &orig, &mut candidates);
        }
    }
    for class_value in [
        "pageTitle",
        "news_title",
        "title",
        "head",
        "heading",
        "contentheading",
        "small_header_red",
    ] {
        for e in elements_with_class_token(doc, class_value) {
            collect_candidate(doc, e, &orig, &mut candidates);
        }
    }

    if !candidates.is_empty() {
        // `sorted(candidates, key=len)[-1]`: longest wins; among ties,
        // take the *last* one in whatever order `sorted` saw them.
        // Python iterates a `set()` here, which has no defined order,
        // so there's no single "correct" tie-break to reproduce --
        // insertion order (the order candidates were discovered in:
        // h1/h2/h3 first, then the ten id/class patterns) is used
        // instead, which is at least deterministic and reproducible.
        let mut best: Option<&String> = None;
        let mut best_len = 0usize;
        for c in &candidates {
            let l = c.chars().count();
            if best.is_none() || l >= best_len {
                best = Some(c);
                best_len = l;
            }
        }
        title = best.cloned().unwrap_or(title);
    } else {
        let mut broke = false;
        for delimiter in [" | ", " - ", " :: ", " / "] {
            if orig.contains(delimiter) {
                let parts: Vec<&str> = orig.split(delimiter).collect();
                if parts[0].split_whitespace().count() >= 4 {
                    title = parts[0].to_string();
                    broke = true;
                    break;
                } else if parts[parts.len() - 1].split_whitespace().count() >= 4 {
                    title = parts[parts.len() - 1].to_string();
                    broke = true;
                    break;
                }
            }
        }
        if !broke && orig.contains(": ") {
            let parts: Vec<&str> = orig.split(": ").collect();
            if parts[parts.len() - 1].split_whitespace().count() >= 4 {
                title = parts[parts.len() - 1].to_string();
            } else if let Some(idx) = orig.find(": ") {
                title = orig[idx + 2..].to_string();
            }
        }
    }

    let len = title.chars().count();
    if !(len > 15 && len < 150) {
        return orig;
    }
    title
}

/// Port of `get_body`: drop `<script>`/`<link>`/`<style>` subtrees, then
/// serialize `doc`'s `<body>` (or `doc` itself if there isn't one) and
/// run [`clean_attributes`] over the result.
pub fn get_body(doc: &mut Dom) -> String {
    for tag in ["script", "link", "style"] {
        for id in doc.find_all_tag_global(tag) {
            doc.detach(id);
        }
    }
    let root_el = doc.find_first_tag_global("html").unwrap_or(doc.root);
    let target = doc
        .find_all_tag(root_el, "body")
        .into_iter()
        .next()
        .unwrap_or(root_el);
    let raw_html = doc.serialize(target);
    clean_attributes(&raw_html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_entities_replaces_dashes_and_quotes() {
        assert_eq!(normalize_entities("a\u{2014}b\u{2013}c"), "a-b-c");
        assert_eq!(normalize_entities("&mdash;&ndash;"), "--");
        assert_eq!(normalize_entities("\u{00ab}hi\u{00bb}"), "\"hi\"");
        assert_eq!(normalize_entities("a&quot;b"), "a\"b");
    }

    #[test]
    fn norm_title_collapses_and_normalizes() {
        assert_eq!(norm_title("  a\u{2014}b   c  "), "a-b c");
    }

    #[test]
    fn get_title_reads_title_tag() {
        let dom = build_doc(b"<html><head><title>Hello World</title></head><body></body></html>");
        assert_eq!(get_title(&dom), "Hello World");
    }

    #[test]
    fn get_title_no_title_tag() {
        let dom = build_doc(b"<html><body>hi</body></html>");
        assert_eq!(get_title(&dom), "[no-title]");
    }

    #[test]
    fn shorten_title_prefers_h1_matching_title() {
        let dom = build_doc(
            b"<html><head><title>My Great Article - My Great Site</title></head>\
              <body><h1>My Great Article</h1></body></html>",
        );
        let short = shorten_title(&dom);
        assert_eq!(short, "My Great Article");
    }

    #[test]
    fn shorten_title_pipe_delimiter_fallback() {
        let dom = build_doc(
            b"<html><head><title>Some Long Headline Here | Example News Site</title></head>\
              <body><p>no headings here</p></body></html>",
        );
        let short = shorten_title(&dom);
        assert_eq!(short, "Some Long Headline Here");
    }

    #[test]
    fn shorten_title_class_token_pattern() {
        let dom = build_doc(
            b"<html><head><title>A Fairly Long Article Title Indeed</title></head>\
              <body><div class=\"foo pageTitle bar\">A Fairly Long Article Title Indeed</div>\
              </body></html>",
        );
        let short = shorten_title(&dom);
        assert_eq!(short, "A Fairly Long Article Title Indeed");
    }

    #[test]
    fn get_body_strips_script_link_style_and_bad_attrs() {
        let mut dom = build_doc(
            b"<html><head><style>.a{}</style></head><body style=\"color:red\">\
              <script>bad()</script><p width=\"10\">hi</p></body></html>",
        );
        let body = get_body(&mut dom);
        assert!(!body.contains("<script"), "{body}");
        assert!(!body.contains("<style"), "{body}");
        assert!(!body.contains("width"), "{body}");
        assert!(!body.contains("style="), "{body}");
        assert!(body.contains("hi"), "{body}");
    }
}
