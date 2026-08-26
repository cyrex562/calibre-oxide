//! The HTML/OEB -> DOCX spine walker: port of `docx/writer/from_html.py`.
//!
//! **[`TextRun`], [`Block`], and [`Blocks`] are ported so far** (the
//! last of these only partially -- see its own doc comment).
//! `lang_for_tag`, the `Style`/`Stylizer` subclasses (add a
//! `letterSpacing` property/a `KeyError`-tolerant `style()` lookup --
//! already subsumed by [`crate::oeb::polish::style::Style`] and
//! [`crate::oeb::polish::cascade`], see `oeb/polish/style.rs`'s
//! module docs), and `Convert` (the real spine-walking orchestrator)
//! are NOT ported -- see issue #132.
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
//!
//! [`Block`]'s constructor and methods take `&mut StylesManager`/
//! `&mut LinksManager` as explicit parameters rather than storing
//! them as fields (Python's `self.styles_manager`/`self.links_manager`)
//! -- storing `&mut` references as struct fields would tie `Block` to
//! a lifetime and make holding many of them at once (`Blocks`, not
//! ported) an aliasing problem for no benefit, since every call site
//! already has both managers in scope. `Block.style`/`.linked_style`
//! (real `BlockStyle`/`CombinedStyle` objects in Python, read via
//! `.id`) are an id handle (`style: BlockStyleId`) and an
//! `Option<String>` respectively -- both only ever get a real id from
//! `StylesManager.finalize`, not ported, so `Block::serialize` takes
//! the resolved id for `self.style` as an explicit `own_style_id`
//! parameter instead of reading it off a stored object.
//! `Block.float_spec.blocks` (the float's member-block list, used in
//! Python only for `block is self.blocks[0]`/`[-1]` identity checks)
//! isn't needed at all -- `FloatSpec::serialize` (PR #328) already
//! takes `is_first_block`/`is_last_block` as explicit bools, so
//! `Block::serialize` just forwards its own such parameters through.
//! `Block.parent_items` (a back-reference into `Blocks`' own bookkeeping,
//! never read by `Block` itself) isn't ported -- it belongs to
//! `Blocks`, not `Block`, once that container exists.
//! `Block.list_tag`'s second tuple element (the raw CSS `Style` used
//! later by `lists.py`, unported) is dropped -- `list_tag` here is
//! just the html block's [`NodeId`]; `Style` is cheap to reconstruct
//! from a `NodeId` (it's `Copy`), so there's nothing to lose by not
//! storing it early.

use std::collections::{HashMap, HashSet};

use crate::docx::names::{barename, DocxNamespace};
use crate::dom::{Dom, NodeId};
use crate::oeb::polish::style::Style;

use indexmap::{IndexMap, IndexSet};

use super::links::LinksManager;
use super::styles::{BlockStyleId, FloatSpec, StylesManager, TextStyleId};
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

/// Port of `Block`: one `<w:p>` worth of content -- a run of
/// [`TextRun`]s sharing one paragraph style, plus the paragraph-level
/// bookkeeping (bookmarks, page breaks, float/list/numbering
/// properties) `Blocks`/`Convert` (not ported) assign onto it.
#[derive(Debug)]
pub struct Block {
    pub force_not_empty: bool,
    pub bookmarks: IndexSet<String>,
    pub list_tag: Option<NodeId>,
    pub is_first_block: bool,
    pub numbering_id: Option<(u32, u32)>,
    pub html_block: NodeId,
    pub html_tag: String,
    pub float_spec: Option<FloatSpec>,
    pub style: BlockStyleId,
    default_text_style: TextStyleId,
    runs: Vec<TextRun>,
    pub skipped: bool,
    pub linked_style: Option<String>,
    pub page_break_before: bool,
    pub keep_lines: bool,
    pub page_break_after: bool,
    pub keep_next: bool,
    pub block_lang: Option<String>,
}

impl Block {
    /// Port of `Block.__init__`. `namespace` isn't stored (see the
    /// module docs); `styles_manager` is only borrowed for the
    /// duration of this call, not stored either.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        styles_manager: &mut StylesManager,
        dom: &Dom,
        html_block: NodeId,
        style: &Style,
        is_table_cell: bool,
        float_spec: Option<FloatSpec>,
        is_list_item: bool,
        parent_bg: Option<&str>,
    ) -> Self {
        let html_tag = barename(dom.tag(html_block).unwrap_or("")).to_string();
        let block_style =
            styles_manager.create_block_style(Some(style), html_block, is_table_cell, parent_bg);
        let default_text_style = styles_manager.create_text_style(style, false);
        Block {
            force_not_empty: false,
            bookmarks: IndexSet::new(),
            list_tag: if is_list_item { Some(html_block) } else { None },
            is_first_block: false,
            numbering_id: None,
            html_block,
            html_tag,
            float_spec,
            style: block_style,
            default_text_style,
            runs: Vec::new(),
            skipped: false,
            linked_style: None,
            page_break_before: style.get("page-break-before") == "always",
            keep_lines: style.get("page-break-inside") == "avoid",
            page_break_after: false,
            keep_next: false,
            block_lang: None,
        }
    }

    /// Port of `Block.resolve_skipped`: if this block turned out
    /// empty and its html tag's first child element is `next_block`'s
    /// html tag (i.e. this block only ever represented a container
    /// element's own inline content, and that content never
    /// appeared), mark it skipped and hand its `list_tag` down to
    /// `next_block`.
    pub fn resolve_skipped(&mut self, dom: &Dom, next_block: &mut Block) {
        if !self.is_empty() {
            return;
        }
        let first_child_element = dom
            .children(self.html_block)
            .into_iter()
            .find(|&c| dom.tag(c).is_some());
        if first_child_element == Some(next_block.html_block) {
            self.skipped = true;
            if let Some(list_tag) = self.list_tag {
                next_block.list_tag = Some(list_tag);
            }
        }
    }

    /// Port of `Block.add_text`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_text(
        &mut self,
        styles_manager: &mut StylesManager,
        text: &str,
        style: &Style,
        ignore_leading_whitespace: bool,
        html_parent: Option<NodeId>,
        is_parent_style: bool,
        mut bookmark: Option<String>,
        link: Option<LinkTarget>,
        lang: Option<String>,
    ) {
        let ws = style.get("white-space");
        let preserve_whitespace = matches!(ws.as_str(), "pre" | "pre-wrap" | "-o-pre-wrap");
        let ts = styles_manager.create_text_style(style, is_parent_style);
        let reuse = self
            .runs
            .last()
            .is_some_and(|r| r.style == ts && r.link == link && r.lang == lang);
        if !reuse {
            let parent = html_parent.unwrap_or(self.html_block);
            self.runs.push(TextRun::new(ts, parent, lang.clone()));
        }
        let run = self.runs.last_mut().expect("just ensured a run exists");

        let mut text = text.to_string();
        if ignore_leading_whitespace && !preserve_whitespace {
            text = text.trim_start().to_string();
        }
        if preserve_whitespace || ws == "pre-line" {
            for line in text.lines() {
                run.add_text(line, preserve_whitespace, bookmark.take(), link.clone());
                run.add_break("none", None);
            }
        } else {
            run.add_text(&text, preserve_whitespace, bookmark, link);
        }
    }

    /// Port of `Block.add_break`. Reuses [`Self::default_text_style`],
    /// computed once in [`Self::new`], rather than recomputing
    /// `styles_manager.create_text_style(self.html_style)` on every
    /// call as Python does -- `create_text_style` is a pure function
    /// of an unchanging input here, so the two are equivalent, and
    /// this avoids `Block` having to hold a `Style` reference (and
    /// thus a lifetime) just for this.
    pub fn add_break(&mut self, clear: impl Into<String>, bookmark: Option<String>) {
        if self.runs.is_empty() {
            self.runs
                .push(TextRun::new(self.default_text_style, self.html_block, None));
        }
        self.runs
            .last_mut()
            .expect("just ensured a run exists")
            .add_break(clear, bookmark);
    }

    /// Port of `Block.add_image`.
    pub fn add_image(&mut self, drawing: Element, bookmark: Option<String>) {
        if self.runs.is_empty() {
            self.runs
                .push(TextRun::new(self.default_text_style, self.html_block, None));
        }
        self.runs
            .last_mut()
            .expect("just ensured a run exists")
            .add_image(drawing, bookmark);
    }

    /// Port of `Block.is_empty`.
    pub fn is_empty(&self) -> bool {
        if self.force_not_empty {
            return false;
        }
        self.runs.iter().all(TextRun::is_empty)
    }

    /// Port of `Block.serialize`: appends one `<w:p>` into `body`.
    /// `own_style_id`/`is_first_float_block`/`is_last_float_block` are
    /// the module-doc-explained stand-ins for reading `self.style.id`
    /// and `self.float_spec.blocks[0]`/`[-1]` identity directly.
    #[allow(clippy::too_many_arguments)]
    pub fn serialize(
        &self,
        body: &mut Element,
        links_manager: &mut LinksManager,
        names: &DocxNamespace,
        own_style_id: Option<&str>,
        is_first_float_block: bool,
        is_last_float_block: bool,
    ) {
        let p = body.append(Element::new("w:p"));

        let mut end_bookmarks = Vec::new();
        for bmark in &self.bookmarks {
            let bid = links_manager.bookmark_id();
            end_bookmarks.push(bid);
            p.append(
                Element::new("w:bookmarkStart")
                    .attr("w:id", bid.to_string())
                    .attr("w:name", bmark.as_str()),
            );
        }
        if let Some(lang) = &self.block_lang {
            if !lang.is_empty() {
                let rpr = p.append(Element::new("w:rPr"));
                rpr.append(
                    Element::new("w:lang")
                        .attr("w:val", lang.as_str())
                        .attr("w:bidi", lang.as_str())
                        .attr("w:eastAsia", lang.as_str()),
                );
            }
        }

        let ppr = p.append(Element::new("w:pPr"));
        if self.keep_next {
            ppr.append(Element::new("w:keepNext"));
        }
        if let Some(float_spec) = &self.float_spec {
            float_spec.serialize(ppr, is_first_float_block, is_last_float_block);
        }
        if let Some((num_id, ilvl)) = self.numbering_id {
            let numpr = ppr.append(Element::new("w:numPr"));
            numpr.append(Element::new("w:ilvl").attr("w:val", ilvl.to_string()));
            numpr.append(Element::new("w:numId").attr("w:val", num_id.to_string()));
        }
        if let Some(id) = &self.linked_style {
            ppr.append(Element::new("w:pStyle").attr("w:val", id.as_str()));
        } else if let Some(id) = own_style_id {
            if !id.is_empty() {
                ppr.append(Element::new("w:pStyle").attr("w:val", id));
            }
        }
        if self.is_first_block {
            ppr.append(Element::new("w:pageBreakBefore").attr("w:val", "off"));
        } else if self.page_break_before {
            ppr.append(Element::new("w:pageBreakBefore").attr("w:val", "on"));
        }
        if self.keep_lines {
            ppr.append(Element::new("w:keepLines").attr("w:val", "on"));
        }

        for run in &self.runs {
            run.serialize(p, links_manager, names);
        }
        for bid in end_bookmarks {
            p.append(Element::new("w:bookmarkEnd").attr("w:id", bid.to_string()));
        }
    }
}

/// Handle into a [`Blocks`]' `Block` arena. Stands in for Python
/// holding a direct `Block` object reference (`Blocks.block_map`,
/// `Blocks.current_block`, `Blocks.all_blocks`/`.items` all key or
/// store on Python object identity, which Rust has no equivalent of
/// -- same reasoning as `TextStyleId`/`BlockStyleId`, PR #330).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

/// Port of `Blocks` -- **partial**. Ported: `new` (`__init__`),
/// `current_or_new_block`, `end_current_block`, `start_new_block`,
/// the block-only half of `finish_tag` (see below),
/// `delete_block_at`, `begin_item`/`end_item` (`__enter__`/
/// `__exit__` -- Rust has no context-manager equivalent that also
/// allows the caller to keep mutating `Blocks` and other managers
/// mid-scope, so these are just two plain methods `Convert`, not
/// ported, will call explicitly around each spine item), and
/// `apply_page_break_after`/`resolve_language`.
///
/// **Not ported, deliberately**:
/// - Everything involving `Table` (`self.tables`/`self.current_table`,
///   `start_new_table`/`start_new_row`/`start_new_cell`, and
///   `finish_tag`'s `if self.current_table is not None:` half) --
///   `tables.py` isn't ported, so there is no `Table` type to hold.
///   `self.items` here is therefore always `Vec<BlockId>`, never the
///   `Block | Table` union Python's version holds.
/// - `serialize` (`Blocks.serialize`, walking `self.items` and
///   calling each item's own `serialize`) -- `Block::serialize` needs
///   a resolved `own_style_id: Option<&str>` per block (only
///   `StylesManager.finalize`, unported, can supply one) and
///   `is_first_float_block`/`is_last_float_block` per block sharing a
///   `FloatSpec` (Python computes these by walking `float_spec.blocks`,
///   itself populated because `Block`/`FloatSpec` are shared mutable
///   Python objects -- this port's `FloatSpec` is a plain owned,
///   `Clone`, value, not `Rc<RefCell<...>>`, precisely to avoid that
///   pattern, following the same reasoning `StylesManager`'s arena
///   design already established). Building `serialize` before
///   deciding how float-group membership gets tracked (a real design
///   question, not a routine port -- see issue #132's own tracking
///   notes) would mean guessing at an interface with no real caller
///   to validate it against yet.
/// - `block_map` (`dict[Block, int]`, Python's object-identity-keyed
///   cache of a block's position within `self.items`, used only to
///   speed up `delete_block_at`'s lookup) isn't ported at all --
///   [`Self::delete_block_at`] does a plain linear
///   [`Vec::iter`]`.`[`position`](Iterator::position) scan of `items`
///   instead. `items` sizes are a book chapter's worth of paragraphs
///   at most, so this is nowhere near a hot path, and dropping the
///   cache sidesteps both the object-identity-as-dict-key translation
///   problem and a latent staleness risk in Python's own cache (a
///   block's cached position never gets updated when an *earlier*
///   block is deleted and everything after it shifts down).
/// - `Block.parent_items` (see [`Block`]'s own module-level design
///   note) means [`Self::apply_page_break_after`]'s Python condition
///   `next_block.parent_items is block.parent_items is self.items`
///   is unconditionally true here (there is only ever one items list,
///   `self.items`, since table-cell item lists don't exist yet) --
///   simplified accordingly; **revisit this the moment table support
///   lands**, since a real per-cell items list would make the
///   condition meaningfully false again.
#[derive(Debug, Default)]
pub struct Blocks {
    pub top_bookmark: Option<String>,
    blocks: Vec<Block>,
    all_blocks: Vec<BlockId>,
    pos: usize,
    current_block: Option<BlockId>,
    items: Vec<BlockId>,
    open_html_blocks: HashSet<NodeId>,
    html_tag_start_blocks: HashMap<NodeId, BlockId>,
}

impl Blocks {
    /// Port of `Blocks.__init__`. `namespace`/`styles_manager`/
    /// `links_manager` aren't stored -- see the module docs.
    pub fn new() -> Self {
        Blocks::default()
    }

    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0]
    }

    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        &mut self.blocks[id.0]
    }

    pub fn current_block(&self) -> Option<BlockId> {
        self.current_block
    }

    pub fn all_blocks(&self) -> &[BlockId] {
        &self.all_blocks
    }

    pub fn items(&self) -> &[BlockId] {
        &self.items
    }

    /// Port of `Blocks.current_or_new_block`.
    pub fn current_or_new_block(
        &mut self,
        styles_manager: &mut StylesManager,
        dom: &Dom,
        html_tag: NodeId,
        tag_style: &Style,
    ) -> BlockId {
        match self.current_block {
            Some(id) => id,
            None => {
                self.start_new_block(styles_manager, dom, html_tag, tag_style, false, None, false)
            }
        }
    }

    /// Port of `Blocks.end_current_block`. The `self.current_table is
    /// not None and self.current_table.current_row is not None`
    /// branch (adding the block to the current table row instead of
    /// `self.items`) is never taken here -- table support isn't
    /// ported, so there is no `current_table` to check.
    pub fn end_current_block(&mut self) {
        if let Some(id) = self.current_block.take() {
            self.all_blocks.push(id);
            self.items.push(id);
        }
    }

    /// Port of `Blocks.start_new_block`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_new_block(
        &mut self,
        styles_manager: &mut StylesManager,
        dom: &Dom,
        html_block: NodeId,
        style: &Style,
        is_table_cell: bool,
        float_spec: Option<FloatSpec>,
        is_list_item: bool,
    ) -> BlockId {
        let mut parent_bg: Option<String> = None;
        if let Some(parent) = dom.parent(html_block) {
            if self.html_tag_start_blocks.contains_key(&parent) {
                if let Some(ps_id) = styles_manager.style_for_html_block(parent) {
                    if let Some(bg) = &styles_manager.block_style(ps_id).background_color {
                        parent_bg = Some(bg.clone());
                    }
                }
            }
        }
        self.end_current_block();
        let block = Block::new(
            styles_manager,
            dom,
            html_block,
            style,
            is_table_cell,
            float_spec,
            is_list_item,
            parent_bg.as_deref(),
        );
        let id = BlockId(self.blocks.len());
        self.blocks.push(block);
        self.html_tag_start_blocks.insert(html_block, id);
        self.open_html_blocks.insert(html_block);
        self.current_block = Some(id);
        id
    }

    /// Port of the block-only half of `Blocks.finish_tag` -- the
    /// `if self.current_table is not None:` half isn't ported (see
    /// the module docs). `tag_style` stands in for Python reading
    /// `start_block.html_style['page-break-after']` off the cached
    /// block that opened `html_tag` -- that block's own `html_style`
    /// *is* `html_tag`'s style (it was created for this exact tag),
    /// so the caller's fresh `tag_style` for `html_tag` at the point
    /// it closes is the same value.
    pub fn finish_tag(&mut self, html_tag: NodeId, tag_style: &Style) {
        if self.current_block.is_some() && self.open_html_blocks.contains(&html_tag) {
            // Every element added to `open_html_blocks` is added to
            // `html_tag_start_blocks` in the same call (start_new_block),
            // so this lookup always succeeds here -- kept as a real
            // check rather than an unchecked index, matching Python's
            // own `if start_block is not None` guard.
            if self.html_tag_start_blocks.contains_key(&html_tag)
                && tag_style.get("page-break-after") == "always"
            {
                if let Some(id) = self.current_block {
                    self.blocks[id.0].page_break_after = true;
                }
            }
            self.end_current_block();
            self.open_html_blocks.remove(&html_tag);
        }
    }

    /// Port of `Blocks.delete_block_at`. See the module docs for why
    /// there's no `block_map`/`parent_items` here.
    pub fn delete_block_at(&mut self, pos: Option<usize>) {
        let pos = pos.unwrap_or(self.pos);
        let block_id = self.all_blocks.remove(pos);
        if let Some(item_pos) = self.items.iter().position(|&id| id == block_id) {
            self.items.remove(item_pos);
        }
        let (bookmarks, page_break_after, page_break_before) = {
            let block = &self.blocks[block_id.0];
            (
                block.bookmarks.clone(),
                block.page_break_after,
                block.page_break_before,
            )
        };
        if let Some(&next_id) = self.all_blocks.get(pos) {
            let next = &mut self.blocks[next_id.0];
            next.bookmarks.extend(bookmarks);
            next.page_break_after = page_break_after;
            next.page_break_before = page_break_before;
        }
    }

    /// Port of `Blocks.__enter__`.
    pub fn begin_item(&mut self) {
        self.pos = self.all_blocks.len();
    }

    /// Port of `Blocks.__exit__`. `ok` stands in for Python's
    /// `value is None` (no exception raised in the `with` block) --
    /// pass `false` to skip cleanup the way an exception would.
    pub fn end_item(&mut self, ok: bool) {
        if !ok {
            return;
        }
        if let Some(id) = self.current_block.take() {
            self.all_blocks.push(id);
        }
        if self.all_blocks.len() > self.pos && self.blocks[self.all_blocks[self.pos].0].is_empty() {
            self.delete_block_at(Some(self.pos));
        }
        if self.pos > 0 && self.pos < self.all_blocks.len() {
            let id = self.all_blocks[self.pos];
            self.blocks[id.0].page_break_before = true;
            if let Some(bmark) = &self.top_bookmark {
                self.blocks[id.0].bookmarks.insert(bmark.clone());
            }
        }
        self.top_bookmark = None;
    }

    /// Port of `Blocks.apply_page_break_after`. Python's
    /// `next_block.parent_items is block.parent_items is self.items`
    /// guard is unconditionally true here -- see the module docs.
    pub fn apply_page_break_after(&mut self) {
        for i in 0..self.all_blocks.len().saturating_sub(1) {
            if self.blocks[self.all_blocks[i].0].page_break_after {
                let next_id = self.all_blocks[i + 1];
                self.blocks[next_id.0].page_break_before = true;
            }
        }
    }

    /// Port of `Blocks.resolve_language`. Ties in "most common lang
    /// among this block's runs" resolve to whichever lang was first
    /// seen, matching Python's `Counter.most_common(1)` (CPython's
    /// `heapq.nlargest(1, ...)` degenerates to a linear max-scan that
    /// only replaces the current best on a strictly greater count).
    pub fn resolve_language(&mut self, default_lang: &str) {
        for &id in &self.all_blocks {
            let block = &mut self.blocks[id.0];
            let mut counts: IndexMap<Option<String>, usize> = IndexMap::new();
            for run in &block.runs {
                *counts.entry(run.lang.clone()).or_insert(0) += 1;
            }
            if counts.is_empty() {
                continue;
            }
            let mut best_lang: Option<String> = None;
            let mut best_count = 0usize;
            let mut have_best = false;
            for (lang, &count) in &counts {
                if !have_best || count > best_count {
                    best_lang = lang.clone();
                    best_count = count;
                    have_best = true;
                }
            }
            block.block_lang = best_lang.clone();
            for run in &mut block.runs {
                if run.lang == best_lang {
                    run.lang = None;
                }
            }
            if best_lang.as_deref() == Some(default_lang) {
                block.block_lang = None;
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

    use crate::oeb::polish::cascade::{PropertyValue, ResolvedStyles};
    use crate::oeb::polish::style::Profile;
    use std::collections::HashMap;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn resolved_with(entries: &[(NodeId, &[(&str, &str)])]) -> ResolvedStyles {
        let mut style_map = HashMap::new();
        for &(id, props) in entries {
            let mut m = HashMap::new();
            for &(k, v) in props {
                m.insert(k.to_string(), PropertyValue::new(v, None, false));
            }
            style_map.insert(id, m);
        }
        ResolvedStyles {
            style_map,
            pseudo_style_map: HashMap::new(),
        }
    }

    fn find(dom: &Dom, tag: &str) -> NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    fn styles_manager() -> StylesManager {
        StylesManager::new("en")
    }

    #[test]
    fn block_new_reads_page_break_flags_and_html_tag() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(
            p,
            &[
                ("page-break-before", "always"),
                ("page-break-inside", "avoid"),
            ],
        )]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        assert_eq!(block.html_tag, "p");
        assert!(block.page_break_before);
        assert!(block.keep_lines);
        assert!(block.is_empty());
    }

    #[test]
    fn block_new_defaults_page_break_flags_to_false_with_no_css() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        assert!(!block.page_break_before);
        assert!(!block.keep_lines);
    }

    #[test]
    fn block_add_text_makes_it_non_empty() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        block.add_text(
            &mut mgr, "hello", &style, false, None, false, None, None, None,
        );
        assert!(!block.is_empty());
        assert_eq!(block.runs.len(), 1);
    }

    #[test]
    fn block_add_text_reuses_the_last_run_when_style_link_and_lang_match() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        block.add_text(&mut mgr, "a", &style, false, None, false, None, None, None);
        block.add_text(&mut mgr, "b", &style, false, None, false, None, None, None);
        assert_eq!(
            block.runs.len(),
            1,
            "same style/link/lang should append to the existing run"
        );
    }

    #[test]
    fn block_add_text_starts_a_new_run_when_style_differs() {
        let dom = make("<html><body><p>x</p><span>y</span></body></html>");
        let p = find(&dom, "p");
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(span, &[("font-weight", "bold")])]);
        let profile = Profile::default();
        let plain = Style::new(&dom, &resolved, &profile, p);
        let bold = Style::new(&dom, &resolved, &profile, span);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &plain, false, None, false, None);
        block.add_text(&mut mgr, "a", &plain, false, None, false, None, None, None);
        block.add_text(&mut mgr, "b", &bold, false, None, false, None, None, None);
        assert_eq!(block.runs.len(), 2);
    }

    #[test]
    fn block_add_text_preserve_whitespace_splits_multiline_text_into_breaks() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("white-space", "pre")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        block.add_text(
            &mut mgr, "a\nb", &style, false, None, false, None, None, None,
        );
        let run = &block.runs[0];
        // "a" text, break, "b" text, break -- one TextEntry per add_text/add_break call.
        assert_eq!(run.texts.len(), 4);
        assert!(matches!(run.texts[1].item, TextItem::Break { .. }));
        assert!(matches!(run.texts[3].item, TextItem::Break { .. }));
    }

    #[test]
    fn block_add_break_creates_a_run_when_none_exists() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        block.add_break("none", None);
        assert_eq!(block.runs.len(), 1);
        assert!(!block.is_empty());
    }

    #[test]
    fn block_add_image_creates_a_run_when_none_exists() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = Block::new(&mut mgr, &dom, p, &style, false, None, false, None);
        block.add_image(Element::new("w:drawing"), None);
        assert_eq!(block.runs.len(), 1);
    }

    #[test]
    fn block_resolve_skipped_marks_skipped_when_empty_and_first_child_is_next_block() {
        let dom = make("<html><body><p>x</p></body></html>");
        let body = find(&dom, "body");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let body_style = Style::new(&dom, &resolved, &profile, body);
        let p_style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut body_block = Block::new(&mut mgr, &dom, body, &body_style, false, None, true, None);
        let mut p_block = Block::new(&mut mgr, &dom, p, &p_style, false, None, false, None);
        body_block.resolve_skipped(&dom, &mut p_block);
        assert!(body_block.skipped);
        assert_eq!(
            p_block.list_tag,
            Some(body),
            "the skipped block's list_tag is handed down to next_block"
        );
    }

    #[test]
    fn block_resolve_skipped_does_nothing_when_not_empty() {
        let dom = make("<html><body><p>x</p></body></html>");
        let body = find(&dom, "body");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let body_style = Style::new(&dom, &resolved, &profile, body);
        let p_style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut body_block =
            Block::new(&mut mgr, &dom, body, &body_style, false, None, false, None);
        body_block.add_text(
            &mut mgr,
            "hi",
            &body_style,
            false,
            None,
            false,
            None,
            None,
            None,
        );
        let mut p_block = Block::new(&mut mgr, &dom, p, &p_style, false, None, false, None);
        body_block.resolve_skipped(&dom, &mut p_block);
        assert!(!body_block.skipped);
    }

    #[test]
    fn block_resolve_skipped_does_nothing_when_first_child_is_a_different_element() {
        let dom = make("<html><body><p>x</p><p>y</p></body></html>");
        let body = find(&dom, "body");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let body_style = Style::new(&dom, &resolved, &profile, body);
        let p_style = Style::new(&dom, &resolved, &profile, ps[1]);
        let mut mgr = styles_manager();
        let mut body_block =
            Block::new(&mut mgr, &dom, body, &body_style, false, None, false, None);
        // next_block is the *second* <p>, not body's first child.
        let mut p_block = Block::new(&mut mgr, &dom, ps[1], &p_style, false, None, false, None);
        body_block.resolve_skipped(&dom, &mut p_block);
        assert!(!body_block.skipped);
    }

    fn dummy_block(mgr: &mut StylesManager, dom: &Dom, node: NodeId, style: &Style) -> Block {
        Block::new(mgr, dom, node, style, false, None, false, None)
    }

    #[test]
    fn block_serialize_produces_a_paragraph_with_pstyle_and_a_run() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = dummy_block(&mut mgr, &dom, p, &style);
        block.add_text(&mut mgr, "hi", &style, false, None, false, None, None, None);
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), Some("MyStyle"), false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        let pstyle = ppr.children_named("w:pStyle").next().unwrap();
        assert_eq!(pstyle.get("w:val"), Some("MyStyle"));
        assert!(p_el.children_named("w:r").next().is_some());
    }

    #[test]
    fn block_serialize_linked_style_wins_over_own_style_id() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = dummy_block(&mut mgr, &dom, p, &style);
        block.linked_style = Some("Combined1".to_string());
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), Some("OwnStyle"), false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        let pstyle = ppr.children_named("w:pStyle").next().unwrap();
        assert_eq!(pstyle.get("w:val"), Some("Combined1"));
    }

    #[test]
    fn block_serialize_is_first_block_forces_page_break_off() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("page-break-before", "always")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = dummy_block(&mut mgr, &dom, p, &style);
        block.is_first_block = true;
        assert!(block.page_break_before);
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), None, false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        let pbb = ppr.children_named("w:pageBreakBefore").next().unwrap();
        assert_eq!(pbb.get("w:val"), Some("off"));
    }

    #[test]
    fn block_serialize_page_break_before_emits_on_when_not_first_block() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("page-break-before", "always")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let block = dummy_block(&mut mgr, &dom, p, &style);
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), None, false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        let pbb = ppr.children_named("w:pageBreakBefore").next().unwrap();
        assert_eq!(pbb.get("w:val"), Some("on"));
    }

    #[test]
    fn block_serialize_bookmarks_wrap_the_whole_paragraph() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = dummy_block(&mut mgr, &dom, p, &style);
        block.bookmarks.insert("mark1".to_string());
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), None, false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let names: Vec<&str> = p_el
            .children
            .iter()
            .filter_map(|c| match c {
                Child::Element(e) => Some(e.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names[0], "w:bookmarkStart");
        assert_eq!(*names.last().unwrap(), "w:bookmarkEnd");
    }

    #[test]
    fn block_serialize_numbering_id_emits_num_pr() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut block = dummy_block(&mut mgr, &dom, p, &style);
        block.numbering_id = Some((3, 1));
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), None, false, false);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        let numpr = ppr.children_named("w:numPr").next().unwrap();
        assert_eq!(
            numpr.children_named("w:numId").next().unwrap().get("w:val"),
            Some("3")
        );
        assert_eq!(
            numpr.children_named("w:ilvl").next().unwrap().get("w:val"),
            Some("1")
        );
    }

    #[test]
    fn block_serialize_forwards_first_and_last_to_float_spec() {
        let dom = make("<html><body><img/></body></html>");
        let img = dom
            .preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some("img"))
            .unwrap();
        let html_tag = dom
            .preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some("html"))
            .unwrap();
        let resolved = resolved_with(&[(img, &[("float", "left")])]);
        let profile = Profile::default();
        let style = Style::new(&dom, &resolved, &profile, img);
        let float_spec = FloatSpec::from_css(&dom, html_tag, &style);
        let mut mgr = styles_manager();
        let block = Block::new(
            &mut mgr,
            &dom,
            img,
            &style,
            false,
            Some(float_spec),
            false,
            None,
        );
        let mut body = Element::new("w:body");
        let mut lm = links_manager();
        block.serialize(&mut body, &mut lm, &ns(), None, true, true);
        let p_el = body.children_named("w:p").next().unwrap();
        let ppr = p_el.children_named("w:pPr").next().unwrap();
        assert!(ppr.children_named("w:framePr").next().is_some());
    }

    fn plain_style<'a>(
        dom: &'a Dom,
        resolved: &'a ResolvedStyles,
        profile: &'a Profile,
        node: NodeId,
    ) -> Style<'a> {
        Style::new(dom, resolved, profile, node)
    }

    #[test]
    fn blocks_start_new_block_becomes_the_current_block() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        assert_eq!(blocks.current_block(), Some(id));
        assert!(
            blocks.items().is_empty(),
            "not added to items until end_current_block"
        );
    }

    #[test]
    fn current_or_new_block_reuses_the_existing_current_block() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let a = blocks.current_or_new_block(&mut mgr, &dom, p, &style);
        let b = blocks.current_or_new_block(&mut mgr, &dom, p, &style);
        assert_eq!(a, b);
    }

    #[test]
    fn start_new_block_ends_the_previous_current_block_first() {
        let dom = make("<html><body><p>x</p><p>y</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style1 = plain_style(&dom, &resolved, &profile, ps[0]);
        let style2 = plain_style(&dom, &resolved, &profile, ps[1]);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let first = blocks.start_new_block(&mut mgr, &dom, ps[0], &style1, false, None, false);
        let second = blocks.start_new_block(&mut mgr, &dom, ps[1], &style2, false, None, false);
        assert_eq!(blocks.current_block(), Some(second));
        assert_eq!(blocks.items(), &[first]);
        assert_eq!(blocks.all_blocks(), &[first]);
    }

    #[test]
    fn start_new_block_inherits_background_from_a_tracked_parent_block() {
        let dom = make("<html><body><div><p>x</p></div></body></html>");
        let div = find(&dom, "div");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(div, &[("background-color", "#ff0000")])]);
        let profile = Profile::default();
        let div_style = plain_style(&dom, &resolved, &profile, div);
        let p_style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        blocks.start_new_block(&mut mgr, &dom, div, &div_style, false, None, false);
        let p_id = blocks.start_new_block(&mut mgr, &dom, p, &p_style, false, None, false);
        let p_block_style = mgr.block_style(blocks.block(p_id).style);
        assert_eq!(p_block_style.background_color.as_deref(), Some("FF0000"));
    }

    #[test]
    fn start_new_block_does_not_inherit_background_when_parent_was_never_a_block_start() {
        // The parent <div> never went through start_new_block (no
        // html_tag_start_blocks entry), so parent_bg stays None even
        // though it has a real background-color.
        let dom = make("<html><body><div><p>x</p></div></body></html>");
        let div = find(&dom, "div");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(div, &[("background-color", "#ff0000")])]);
        let profile = Profile::default();
        let p_style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let p_id = blocks.start_new_block(&mut mgr, &dom, p, &p_style, false, None, false);
        let p_block_style = mgr.block_style(blocks.block(p_id).style);
        assert_eq!(p_block_style.background_color, None);
    }

    #[test]
    fn end_current_block_moves_current_into_all_blocks_and_items() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.end_current_block();
        assert_eq!(blocks.current_block(), None);
        assert_eq!(blocks.items(), &[id]);
        assert_eq!(blocks.all_blocks(), &[id]);
    }

    #[test]
    fn finish_tag_sets_page_break_after_when_style_says_always() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[(p, &[("page-break-after", "always")])]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.finish_tag(p, &style);
        assert!(blocks.block(id).page_break_after);
        assert_eq!(
            blocks.current_block(),
            None,
            "finish_tag ends the current block"
        );
    }

    #[test]
    fn finish_tag_leaves_page_break_after_false_without_always() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.finish_tag(p, &style);
        assert!(!blocks.block(id).page_break_after);
    }

    #[test]
    fn finish_tag_is_a_no_op_for_a_tag_that_was_never_opened() {
        let dom = make("<html><body><p>x</p><p>y</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, ps[0]);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        blocks.start_new_block(&mut mgr, &dom, ps[0], &style, false, None, false);
        // ps[1] was never opened as a block start.
        blocks.finish_tag(ps[1], &style);
        assert!(
            blocks.current_block().is_some(),
            "unrelated tag must not end the real current block"
        );
    }

    #[test]
    fn delete_block_at_removes_and_transfers_bookmarks_and_page_breaks_to_the_next_block() {
        let dom = make("<html><body><p>x</p><p>y</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style1 = plain_style(&dom, &resolved, &profile, ps[0]);
        let style2 = plain_style(&dom, &resolved, &profile, ps[1]);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let first = blocks.start_new_block(&mut mgr, &dom, ps[0], &style1, false, None, false);
        blocks
            .block_mut(first)
            .bookmarks
            .insert("mark1".to_string());
        blocks.block_mut(first).page_break_after = true;
        blocks.end_current_block();
        let second = blocks.start_new_block(&mut mgr, &dom, ps[1], &style2, false, None, false);
        blocks.end_current_block();
        blocks.delete_block_at(Some(0));
        assert_eq!(blocks.all_blocks(), &[second]);
        assert_eq!(blocks.items(), &[second]);
        assert!(blocks.block(second).bookmarks.contains("mark1"));
        assert!(blocks.block(second).page_break_after);
    }

    #[test]
    fn begin_end_item_deletes_an_empty_leading_block_and_marks_the_next_page_break() {
        let dom = make("<html><body><p></p><p>y</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style1 = plain_style(&dom, &resolved, &profile, ps[0]);
        let style2 = plain_style(&dom, &resolved, &profile, ps[1]);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        // A prior real item already put one block in all_blocks, so
        // begin_item's `pos` is non-zero -- matching Python's own
        // "insert a page break at the start of this html file" case.
        let prior = blocks.start_new_block(&mut mgr, &dom, ps[0], &style1, false, None, false);
        blocks.end_current_block();
        blocks.begin_item();
        blocks.top_bookmark = Some("top1".to_string());
        // Empty block (never got any text) for this "item".
        blocks.start_new_block(&mut mgr, &dom, ps[0], &style1, false, None, false);
        blocks.end_current_block();
        let real = blocks.start_new_block(&mut mgr, &dom, ps[1], &style2, false, None, false);
        blocks
            .block_mut(real)
            .add_text(&mut mgr, "y", &style2, false, None, false, None, None, None);
        blocks.end_current_block();
        blocks.end_item(true);
        assert_eq!(blocks.all_blocks(), &[prior, real]);
        assert!(blocks.block(real).page_break_before);
        assert!(blocks.block(real).bookmarks.contains("top1"));
        assert_eq!(blocks.top_bookmark, None);
    }

    #[test]
    fn end_item_with_ok_false_does_nothing() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.begin_item();
        blocks.top_bookmark = Some("keep-me".to_string());
        blocks.end_item(false);
        assert!(
            blocks.current_block().is_some(),
            "current_block untouched when ok is false"
        );
        assert_eq!(blocks.top_bookmark, Some("keep-me".to_string()));
    }

    #[test]
    fn apply_page_break_after_propagates_to_the_next_block_only() {
        let dom = make("<html><body><p>a</p><p>b</p><p>c</p></body></html>");
        let ps: Vec<NodeId> = dom
            .preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some("p"))
            .collect();
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let mut ids = Vec::new();
        for &p in &ps {
            let style = plain_style(&dom, &resolved, &profile, p);
            ids.push(blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false));
            blocks.end_current_block();
        }
        blocks.block_mut(ids[1]).page_break_after = true;
        blocks.apply_page_break_after();
        assert!(!blocks.block(ids[0]).page_break_before);
        assert!(!blocks.block(ids[1]).page_break_before);
        assert!(blocks.block(ids[2]).page_break_before);
    }

    #[test]
    fn apply_page_break_after_on_the_last_block_does_nothing() {
        let dom = make("<html><body><p>a</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.end_current_block();
        blocks.block_mut(id).page_break_after = true;
        blocks.apply_page_break_after();
        // No panic and nothing to assert beyond "didn't crash" -- the
        // real assertion is that this doesn't index out of bounds.
    }

    fn add_lang_run(
        blocks: &mut Blocks,
        mgr: &mut StylesManager,
        id: BlockId,
        style: &Style,
        lang: Option<&str>,
    ) {
        blocks.block_mut(id).add_text(
            mgr,
            "x",
            style,
            false,
            None,
            false,
            None,
            None,
            lang.map(str::to_string),
        );
    }

    #[test]
    fn resolve_language_sets_block_lang_to_the_most_common_and_clears_matching_runs() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        // Two runs tagged "de", one "fr" -- "de" wins.
        add_lang_run(&mut blocks, &mut mgr, id, &style, Some("de"));
        blocks.block_mut(id).add_break("none", None);
        add_lang_run(&mut blocks, &mut mgr, id, &style, Some("fr"));
        blocks.block_mut(id).add_break("none", None);
        add_lang_run(&mut blocks, &mut mgr, id, &style, Some("de"));
        blocks.end_current_block();
        blocks.resolve_language("en");
        assert_eq!(blocks.block(id).block_lang.as_deref(), Some("de"));
        for run in &blocks.block(id).runs {
            if run.lang.as_deref() == Some("fr") {
                continue;
            }
            assert_eq!(run.lang, None, "runs matching the winning lang get cleared");
        }
    }

    #[test]
    fn resolve_language_clears_block_lang_when_it_matches_the_document_default() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        add_lang_run(&mut blocks, &mut mgr, id, &style, Some("en"));
        blocks.end_current_block();
        blocks.resolve_language("en");
        assert_eq!(blocks.block(id).block_lang, None);
    }

    #[test]
    fn resolve_language_skips_a_block_with_no_runs() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let style = plain_style(&dom, &resolved, &profile, p);
        let mut mgr = styles_manager();
        let mut blocks = Blocks::new();
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.end_current_block();
        blocks.resolve_language("en");
        assert_eq!(blocks.block(id).block_lang, None);
    }
}
