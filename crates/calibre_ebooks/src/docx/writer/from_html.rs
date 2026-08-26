//! The HTML/OEB -> DOCX spine walker: port of `docx/writer/from_html.py`.
//!
//! **Only [`TextRun`] is ported so far** -- the leaf type with the
//! fewest cross-dependencies (needs [`super::styles::TextStyleId`]
//! and [`super::links::LinksManager`], both already ported; needs
//! nothing from `Block`/`Blocks`/`Convert`, none of which exist yet).
//! `lang_for_tag`, the `Style`/`Stylizer` subclasses (add a
//! `letterSpacing` property/a `KeyError`-tolerant `style()` lookup --
//! already subsumed by [`crate::oeb::polish::style::Style`] and
//! [`crate::oeb::polish::cascade`], see `oeb/polish/style.rs`'s
//! module docs), `Block`, `Blocks`, and `Convert` (the actual
//! spine-walking orchestrator) are NOT ported -- see issue #132.
//!
//! `TextRun.first_html_parent` (an lxml element in Python) is a
//! [`NodeId`] here; `TextRun.style`/`.parent_style` (a shared
//! `TextStyle` object reference in Python) are [`TextStyleId`]
//! handles into a [`super::styles::StylesManager`] arena, matching
//! that module's own already-established design (issue #132, PR
//! #330). `TextRun.descendant_style` (a shared `DescendantTextStyle`
//! object in Python, only ever read via `.id`) is stored here as
//! just that `id: Option<String>` directly -- `StylesManager.finalize`,
//! which assigns real ids to deduplicated descendant styles, isn't
//! ported, so there is no id-carrying object to reference yet.
//! `TextRun.link` (a raw `(item, url, tooltip)` tuple in Python) is
//! [`LinkTarget`], storing the link's `current_item`'s href directly
//! rather than a whole manifest `Item` object, matching
//! `LinksManager::serialize_hyperlink`'s own already-established
//! `current_item_href: &str` parameter (issue #132, PR #331).

use crate::docx::names::DocxNamespace;
use crate::dom::NodeId;

use super::links::LinksManager;
use super::styles::TextStyleId;
use super::xml::{Child, Element};

/// Port of the `(item, url, tooltip)` tuple Python's
/// `TextRun.link`/`Block.add_text`'s `link` parameter pass around.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkTarget {
    pub item_href: String,
    pub url: String,
    pub tooltip: Option<String>,
}

/// One entry of `TextRun.texts`. Python stores these as an untyped
/// 3-tuple whose second slot means different things depending on the
/// first (a bool for text, a `clear` keyword string for a break,
/// unused for an image) -- ported as a real enum instead of
/// reproducing that positional overloading.
#[derive(Debug, Clone, PartialEq)]
enum TextItem {
    Text {
        text: String,
        preserve_whitespace: bool,
    },
    Break {
        clear: String,
    },
    Image {
        drawing: Element,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct TextEntry {
    item: TextItem,
    bookmark: Option<String>,
}

/// Port of `TextRun`: one `<w:r>` (or, for text containing a soft
/// hyphen, several sibling runs sharing one `<w:rPr>`) worth of
/// content sharing one character style.
#[derive(Debug, Clone)]
pub struct TextRun {
    pub first_html_parent: NodeId,
    pub style: TextStyleId,
    texts: Vec<TextEntry>,
    pub link: Option<LinkTarget>,
    pub lang: Option<String>,
    pub parent_style: Option<TextStyleId>,
    pub descendant_style_id: Option<String>,
}

/// Port of `TextRun.ws_pat.sub(' ', text)`: collapses every run of
/// Unicode whitespace to a single space, matching Python's default
/// (non-`re.ASCII`) `\s+`.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
            }
            last_was_ws = true;
        } else {
            out.push(ch);
            last_was_ws = false;
        }
    }
    out
}

/// Port of `self.soft_hyphen_pat.split(text)` (`re.compile(r'(\xad)')`,
/// a capturing-group split, so the delimiter itself is interleaved
/// into the result). The delimiter is a single fixed character, so
/// this is a plain manual split rather than a general regex split.
fn split_keep_soft_hyphen(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if ch == '\u{ad}' {
            parts.push(&text[start..idx]);
            parts.push(&text[idx..idx + ch.len_utf8()]);
            start = idx + ch.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
}

fn last_child_tag(r: &Element) -> Option<&str> {
    match r.children.last() {
        Some(Child::Element(e)) => Some(e.name.as_str()),
        _ => None,
    }
}

/// A mutable reference to the last child's own text content, but only
/// when that last child is a `w:t` element -- matching Python's
/// `r[-1].text` (every other element this run ever appends leaves
/// `.text` unset, so "has text" and "is a `w:t`" coincide here).
fn last_wt_text_mut(r: &mut Element) -> Option<&mut String> {
    match r.children.last_mut() {
        Some(Child::Element(e)) if e.name == "w:t" => match e.children.first_mut() {
            Some(Child::Text(t)) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// Port of the `add_text` closure inside `TextRun.serialize`.
fn append_wt(r: &mut Element, text: &str, preserve_whitespace: bool) {
    let mut t = Element::new("w:t").with_text(text.to_string());
    if preserve_whitespace {
        t = t.attr("xml:space", "preserve");
    }
    r.append(t);
}

impl TextRun {
    /// Port of `TextRun.__init__`. `namespace` isn't stored -- it was
    /// only ever used for `Namespace.makeelement`, which [`Element`]
    /// doesn't need.
    pub fn new(style: TextStyleId, first_html_parent: NodeId, lang: Option<String>) -> Self {
        TextRun {
            first_html_parent,
            style,
            texts: Vec::new(),
            link: None,
            lang,
            parent_style: None,
            descendant_style_id: None,
        }
    }

    /// Port of `TextRun.add_text`. Collapsing internal whitespace can
    /// itself expose a leading/trailing space that Word would
    /// otherwise eat, which is why `preserve_whitespace` can flip
    /// from `false` to `true` here even though the caller asked for
    /// `false`.
    pub fn add_text(
        &mut self,
        text: &str,
        mut preserve_whitespace: bool,
        bookmark: Option<String>,
        link: Option<LinkTarget>,
    ) {
        let text = if preserve_whitespace {
            text.to_string()
        } else {
            let collapsed = collapse_whitespace(text);
            if collapsed.trim() != collapsed {
                preserve_whitespace = true;
            }
            collapsed
        };
        self.texts.push(TextEntry {
            item: TextItem::Text {
                text,
                preserve_whitespace,
            },
            bookmark,
        });
        self.link = link;
    }

    /// Port of `TextRun.add_break`.
    pub fn add_break(&mut self, clear: impl Into<String>, bookmark: Option<String>) {
        self.texts.push(TextEntry {
            item: TextItem::Break {
                clear: clear.into(),
            },
            bookmark,
        });
    }

    /// Port of `TextRun.add_image`.
    pub fn add_image(&mut self, drawing: Element, bookmark: Option<String>) {
        self.texts.push(TextEntry {
            item: TextItem::Image { drawing },
            bookmark,
        });
    }

    /// Port of `TextRun.is_empty`.
    pub fn is_empty(&self) -> bool {
        match self.texts.as_slice() {
            [] => true,
            [entry] => matches!(
                &entry.item,
                TextItem::Text { text, preserve_whitespace } if text.is_empty() && !preserve_whitespace
            ),
            _ => false,
        }
    }

    /// Port of the `style_weight` property: the combined length of
    /// every real text chunk (breaks and images don't count).
    pub fn style_weight(&self) -> usize {
        self.texts
            .iter()
            .map(|e| match &e.item {
                TextItem::Text { text, .. } => text.chars().count(),
                _ => 0,
            })
            .sum()
    }

    /// Port of `TextRun.serialize`: appends one `<w:r>` (wrapped in a
    /// `<w:hyperlink>` if `self.link` is set) into `p`.
    pub fn serialize(
        &self,
        p: &mut Element,
        links_manager: &mut LinksManager,
        names: &DocxNamespace,
    ) {
        let parent: &mut Element = match &self.link {
            None => p,
            Some(link) => links_manager.serialize_hyperlink(
                p,
                names,
                &link.item_href,
                &link.url,
                link.tooltip.as_deref(),
            ),
        };
        let r = parent.append(Element::new("w:r"));

        let mut rpr = Element::new("w:rPr");
        if let Some(id) = &self.descendant_style_id {
            rpr.append(Element::new("w:rStyle").attr("w:val", id));
        }
        if let Some(lang) = &self.lang {
            if !lang.is_empty() {
                rpr.append(
                    Element::new("w:lang")
                        .attr("w:bidi", lang.as_str())
                        .attr("w:val", lang.as_str())
                        .attr("w:eastAsia", lang.as_str()),
                );
            }
        }
        if !rpr.is_empty() {
            r.append(rpr);
        }

        for entry in &self.texts {
            let bookmark_id = entry.bookmark.as_ref().map(|name| {
                let bid = links_manager.bookmark_id();
                r.append(
                    Element::new("w:bookmarkStart")
                        .attr("w:id", bid.to_string())
                        .attr("w:name", name.as_str()),
                );
                bid
            });

            match &entry.item {
                TextItem::Break { clear } => {
                    r.append(Element::new("w:br").attr("w:clear", clear.as_str()));
                }
                TextItem::Image { drawing } => {
                    r.append(drawing.clone());
                }
                TextItem::Text {
                    text,
                    preserve_whitespace,
                } => {
                    if text.is_empty() {
                        append_wt(r, "", *preserve_whitespace);
                    } else {
                        for x in split_keep_soft_hyphen(text) {
                            if x == "\u{ad}" {
                                if !preserve_whitespace {
                                    let needs_space_fix = last_wt_text_mut(r)
                                        .map(|t| t.ends_with(' '))
                                        .unwrap_or(false);
                                    if needs_space_fix {
                                        if let Some(t) = last_wt_text_mut(r) {
                                            *t = t.trim_end().to_string();
                                        }
                                        append_wt(r, " ", true);
                                    }
                                }
                                r.append(Element::new("w:softHyphen"));
                            } else if !x.is_empty() {
                                let mut x = x.to_string();
                                if !preserve_whitespace
                                    && x.starts_with(' ')
                                    && last_child_tag(r) == Some("w:softHyphen")
                                {
                                    x = x.trim_start().to_string();
                                    append_wt(r, " ", true);
                                }
                                append_wt(r, &x, *preserve_whitespace);
                            }
                        }
                    }
                }
            }

            if let Some(bid) = bookmark_id {
                r.append(Element::new("w:bookmarkEnd").attr("w:id", bid.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::writer::container::DocumentRelationships;
    use crate::dom::Dom;

    fn some_node() -> NodeId {
        let dom = Dom::parse("<html><body><p>x</p></body></html>");
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some("p"))
            .unwrap()
    }

    fn run() -> TextRun {
        TextRun::new(TextStyleId(0), some_node(), None)
    }

    #[test]
    fn is_empty_with_no_texts() {
        assert!(run().is_empty());
    }

    #[test]
    fn is_empty_with_a_single_empty_non_preserved_text() {
        let mut r = run();
        r.add_text("", false, None, None);
        assert!(r.is_empty());
    }

    #[test]
    fn not_empty_with_a_single_preserved_empty_text() {
        let mut r = run();
        r.add_text("", true, None, None);
        assert!(!r.is_empty());
    }

    #[test]
    fn not_empty_with_real_text() {
        let mut r = run();
        r.add_text("hello", false, None, None);
        assert!(!r.is_empty());
    }

    #[test]
    fn not_empty_with_two_entries_even_if_both_are_blank() {
        let mut r = run();
        r.add_text("", false, None, None);
        r.add_break("none", None);
        assert!(!r.is_empty());
    }

    #[test]
    fn add_text_collapses_internal_whitespace_runs() {
        let mut r = run();
        r.add_text("a   b\n\tc", false, None, None);
        match &r.texts[0].item {
            TextItem::Text {
                text,
                preserve_whitespace,
            } => {
                assert_eq!(text, "a b c");
                assert!(!preserve_whitespace);
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn add_text_forces_preserve_whitespace_when_leading_or_trailing_space_survives() {
        let mut r = run();
        r.add_text(" hello ", false, None, None);
        match &r.texts[0].item {
            TextItem::Text {
                text,
                preserve_whitespace,
            } => {
                assert_eq!(text, " hello ");
                assert!(
                    preserve_whitespace,
                    "leading/trailing space must be preserved or Word eats it"
                );
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn add_text_with_preserve_whitespace_true_skips_collapsing() {
        let mut r = run();
        r.add_text("a   b", true, None, None);
        match &r.texts[0].item {
            TextItem::Text { text, .. } => assert_eq!(text, "a   b"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn add_text_sets_the_run_link_and_later_calls_overwrite_it() {
        let mut r = run();
        let link = LinkTarget {
            item_href: "chap1.html".to_string(),
            url: "chap2.html".to_string(),
            tooltip: None,
        };
        r.add_text("a", false, None, Some(link.clone()));
        assert_eq!(r.link, Some(link));
        r.add_text("b", false, None, None);
        assert_eq!(r.link, None);
    }

    #[test]
    fn style_weight_counts_only_text_chars() {
        let mut r = run();
        r.add_text("hello", false, None, None);
        r.add_break("none", None);
        r.add_image(Element::new("w:drawing"), None);
        r.add_text("!!", false, None, None);
        assert_eq!(r.style_weight(), 7);
    }

    fn ns() -> DocxNamespace {
        DocxNamespace::new(true)
    }

    fn links_manager() -> LinksManager {
        LinksManager::new(DocumentRelationships::new(&ns()))
    }

    #[test]
    fn serialize_plain_text_produces_one_w_t() {
        let mut r = run();
        r.add_text("hello", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let t = run_el.children_named("w:t").next().unwrap();
        assert_eq!(t.children, vec![Child::Text("hello".to_string())]);
        assert!(t.get("xml:space").is_none());
    }

    #[test]
    fn serialize_preserved_whitespace_sets_xml_space() {
        let mut r = run();
        r.add_text(" hi ", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let t = run_el.children_named("w:t").next().unwrap();
        assert_eq!(t.get("xml:space"), Some("preserve"));
    }

    #[test]
    fn serialize_break_emits_w_br_with_clear() {
        let mut r = run();
        r.add_break("left", None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let br = run_el.children_named("w:br").next().unwrap();
        assert_eq!(br.get("w:clear"), Some("left"));
    }

    #[test]
    fn serialize_image_appends_the_drawing_element_verbatim() {
        let mut r = run();
        r.add_image(Element::new("w:drawing").attr("id", "d1"), None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let drawing = run_el.children_named("w:drawing").next().unwrap();
        assert_eq!(drawing.get("id"), Some("d1"));
    }

    #[test]
    fn serialize_bookmark_wraps_the_content_in_start_and_end() {
        let mut r = run();
        r.add_text("hi", false, Some("mark1".to_string()), None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let names: Vec<&str> = run_el
            .children
            .iter()
            .map(|c| match c {
                Child::Element(e) => e.name.as_str(),
                Child::Text(_) => "",
            })
            .collect();
        assert_eq!(names, vec!["w:bookmarkStart", "w:t", "w:bookmarkEnd"]);
        let start = run_el.children_named("w:bookmarkStart").next().unwrap();
        assert_eq!(start.get("w:name"), Some("mark1"));
        assert_eq!(start.get("w:id"), Some("1"));
        let end = run_el.children_named("w:bookmarkEnd").next().unwrap();
        assert_eq!(end.get("w:id"), Some("1"));
    }

    #[test]
    fn serialize_lang_emits_w_lang_on_all_three_slots() {
        let style = TextStyleId(0);
        let mut r = TextRun::new(style, some_node(), Some("de".to_string()));
        r.add_text("hallo", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let rpr = run_el.children_named("w:rPr").next().unwrap();
        let lang = rpr.children_named("w:lang").next().unwrap();
        assert_eq!(lang.get("w:bidi"), Some("de"));
        assert_eq!(lang.get("w:val"), Some("de"));
        assert_eq!(lang.get("w:eastAsia"), Some("de"));
    }

    #[test]
    fn serialize_descendant_style_id_emits_r_style() {
        let mut r = run();
        r.descendant_style_id = Some("Text0".to_string());
        r.add_text("hi", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let rpr = run_el.children_named("w:rPr").next().unwrap();
        let rstyle = rpr.children_named("w:rStyle").next().unwrap();
        assert_eq!(rstyle.get("w:val"), Some("Text0"));
    }

    #[test]
    fn serialize_with_no_lang_or_descendant_style_omits_rpr() {
        let mut r = run();
        r.add_text("hi", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        assert!(run_el.children_named("w:rPr").next().is_none());
    }

    #[test]
    fn serialize_link_wraps_the_run_in_a_hyperlink() {
        let mut r = run();
        r.add_text(
            "click",
            false,
            None,
            Some(LinkTarget {
                item_href: "chap1.html".to_string(),
                url: "https://example.com/".to_string(),
                tooltip: None,
            }),
        );
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        assert!(
            p.children_named("w:r").next().is_none(),
            "the run is nested inside the hyperlink, not a direct child of p"
        );
        let hl = p.children_named("w:hyperlink").next().unwrap();
        assert!(hl.children_named("w:r").next().is_some());
    }

    #[test]
    fn serialize_soft_hyphen_splits_into_sibling_runs() {
        let mut r = run();
        r.add_text("foo\u{ad}bar", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let tags: Vec<&str> = run_el
            .children
            .iter()
            .filter_map(|c| match c {
                Child::Element(e) => Some(e.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(tags, vec!["w:t", "w:softHyphen", "w:t"]);
        let texts: Vec<_> = run_el.children_named("w:t").collect();
        assert_eq!(texts[0].children, vec![Child::Text("foo".to_string())]);
        assert_eq!(texts[1].children, vec![Child::Text("bar".to_string())]);
    }

    #[test]
    fn serialize_soft_hyphen_preserves_a_trailing_space_before_it() {
        // "foo \xad bar": the space right before the soft hyphen would
        // otherwise be silently eaten by Word, so it gets rstripped off
        // the preceding w:t and re-added as its own preserved-space run.
        let mut r = run();
        r.add_text("foo \u{ad}bar", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let wt: Vec<_> = run_el.children_named("w:t").collect();
        // "foo" (rstripped), then a preserved " ", then "bar" -- with
        // w:softHyphen sandwiched between the second and third.
        assert_eq!(wt[0].children, vec![Child::Text("foo".to_string())]);
        assert_eq!(wt[1].children, vec![Child::Text(" ".to_string())]);
        assert_eq!(wt[1].get("xml:space"), Some("preserve"));
        assert_eq!(wt[2].children, vec![Child::Text("bar".to_string())]);
    }

    #[test]
    fn serialize_soft_hyphen_preserves_a_leading_space_after_it() {
        let mut r = run();
        r.add_text("foo\u{ad} bar", false, None, None);
        let mut p = Element::new("w:p");
        let mut lm = links_manager();
        r.serialize(&mut p, &mut lm, &ns());
        let run_el = p.children_named("w:r").next().unwrap();
        let wt: Vec<_> = run_el.children_named("w:t").collect();
        assert_eq!(wt[0].children, vec![Child::Text("foo".to_string())]);
        assert_eq!(wt[1].children, vec![Child::Text(" ".to_string())]);
        assert_eq!(wt[1].get("xml:space"), Some("preserve"));
        assert_eq!(wt[2].children, vec![Child::Text("bar".to_string())]);
    }
}
