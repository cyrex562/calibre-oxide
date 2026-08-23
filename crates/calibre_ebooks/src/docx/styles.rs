//! Page properties, named `<w:style>` entries, and the
//! `docDefaults -> named style -> direct formatting` cascade that
//! resolves them into per-paragraph/per-run [`ParagraphStyle`]s and
//! [`RunStyle`]s ([`Styles`]).
//!
//! Port of `old_src/src/calibre/ebooks/docx/styles.py`, minus two
//! pieces still blocked (see issue #130): [`Styles::cascade`] needs
//! an `is-link` marker and a paragraph -> runs `layers` map that only
//! exist once `to_html.rs`'s real port builds the HTML body, and
//! [`Styles::resolve_run`]'s font resolution stops at
//! [`super::theme::Theme::resolve_font_family`] rather than also
//! matching against the system's installed fonts
//! (`fonts.py`'s `Fonts::family_for`, which needs a font scanner with
//! no Rust counterpart yet) -- `generate_css` (which needs
//! `Fonts::embed_fonts`) is deferred for the same reason.
//!
//! # `calibre_num_id`: a tracked map, not a synthetic attribute
//!
//! Python's `resolve_paragraph` calls `p.set('calibre_num_id',
//! f'{lvl}:{num_id}')` on the *source* `w:p` element -- a second
//! instance of the source-tree-mutation pattern `docx::tables`'
//! module docs already cover for `handle_merged_cells`, and read back
//! later by `to_html.py` (`obj.get('calibre_num_id', None)`) to know
//! which paragraphs are numbered without re-deriving it. Represented
//! here as [`Styles::calibre_num_ids`], a plain `HashMap` from the
//! paragraph node to its `(level, numbering_id)`, for the same reason
//! `removed_cells` replaced real tree mutation there: every other
//! reader in this crate depends on the source tree staying read-only
//! `roxmltree`.

use std::collections::{BTreeMap, HashMap};

use indexmap::IndexMap;
use roxmltree::Node;

use super::block_styles::{twips, Css, ParagraphStyle};
use super::char_styles::RunStyle;
use super::names::DocxNamespace;
use super::numbering::Numbering;
use super::tables::{TableStyle, Tables};
use super::theme::Theme;

/// Page size/margins, read from `w:sectPr` elements. Defaults to A4
/// with 1in margins, Word's own defaults when nothing is specified.
///
/// Port of the Python `PageProperties`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageProperties {
    pub width: f64,
    pub height: f64,
    pub margin_left: f64,
    pub margin_right: f64,
}

impl Default for PageProperties {
    fn default() -> Self {
        Self {
            width: 595.28,
            height: 841.89,
            margin_left: 72.0,
            margin_right: 72.0,
        }
    }
}

impl PageProperties {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `PageProperties(namespace, elems)`.
    pub fn from_sect_prs(elems: &[Node], ns: &DocxNamespace) -> Self {
        let mut p = Self::new();
        for &sect_pr in elems {
            for pg_sz in ns.children(sect_pr, &["w:pgSz"]) {
                if let Some(v) = twips(ns.get(pg_sz, "w:w"), 0.05) {
                    p.width = v;
                }
                if let Some(v) = twips(ns.get(pg_sz, "w:h"), 0.05) {
                    p.height = v;
                }
            }
            for pg_mar in ns.children(sect_pr, &["w:pgMar"]) {
                if let Some(v) = twips(ns.get(pg_mar, "w:left"), 0.05) {
                    p.margin_left = v;
                }
                if let Some(v) = twips(ns.get(pg_mar, "w:right"), 0.05) {
                    p.margin_right = v;
                }
            }
        }
        p
    }
}

/// The last of `elem`'s direct children matching `qname` that also
/// carries a `w:val` attribute -- the `./w:x[@w:val]` XPath idiom used
/// throughout `styles.py`, taking the *last* match (Python iterates
/// and keeps reassigning).
fn last_child_with_val<'a, 'i>(
    elem: Node<'a, 'i>,
    ns: &DocxNamespace,
    qname: &str,
) -> Option<Node<'a, 'i>> {
    ns.children(elem, &[qname])
        .into_iter()
        .filter(|n| ns.get(*n, "w:val").is_some())
        .last()
}

/// The first such child, for the one Python spot that takes `[0]`
/// instead of the last (`based_on`).
fn first_child_with_val<'a, 'i>(
    elem: Node<'a, 'i>,
    ns: &DocxNamespace,
    qname: &str,
) -> Option<Node<'a, 'i>> {
    ns.children(elem, &[qname])
        .into_iter()
        .find(|n| ns.get(*n, "w:val").is_some())
}

/// One `<w:style>` entry -- a named paragraph, character, table or
/// numbering style, and (via [`Style::resolve_based_on`]) its
/// `w:basedOn` inheritance chain fully resolved.
///
/// Port of the Python `Style`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub resolved: bool,
    pub style_id: Option<String>,
    pub style_type: Option<String>,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub is_default: bool,
    pub paragraph_style: Option<ParagraphStyle>,
    pub character_style: Option<RunStyle>,
    pub table_style: Option<TableStyle>,
    /// Only meaningful when `style_type` is `"numbering"` or
    /// `"paragraph"`; `None` for every other style type (mirroring
    /// the Python attribute simply not existing on those instances).
    pub numbering_style_link: Option<String>,
}

impl Style {
    /// Port of `Style(namespace, elem)`.
    pub fn from_elem(elem: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::default();
        s.style_id = ns.get(elem, "w:styleId").map(str::to_string);
        s.style_type = ns.get(elem, "w:type").map(str::to_string);
        s.name = last_child_with_val(elem, ns, "w:name")
            .and_then(|n| ns.get(n, "w:val"))
            .map(str::to_string);
        s.based_on = first_child_with_val(elem, ns, "w:basedOn")
            .and_then(|n| ns.get(n, "w:val"))
            .map(str::to_string);
        if s.style_type.as_deref() == Some("numbering") {
            s.based_on = None;
        }
        s.is_default = matches!(
            ns.get(elem, "w:default"),
            Some("1") | Some("on") | Some("true")
        );

        if matches!(
            s.style_type.as_deref(),
            Some("paragraph" | "character" | "table")
        ) {
            if s.style_type.as_deref() == Some("table") {
                for tblpr in ns.children(elem, &["w:tblPr"]) {
                    let ts = TableStyle::from_tblpr(tblpr, ns);
                    match &mut s.table_style {
                        None => s.table_style = Some(ts),
                        Some(existing) => existing.update(&ts),
                    }
                }
            }
            if matches!(s.style_type.as_deref(), Some("paragraph" | "table")) {
                for ppr in ns.children(elem, &["w:pPr"]) {
                    let ps = ParagraphStyle::from_ppr(ppr, ns);
                    match &mut s.paragraph_style {
                        None => s.paragraph_style = Some(ps),
                        Some(existing) => existing.update(&ps),
                    }
                }
            }
            for rpr in ns.children(elem, &["w:rPr"]) {
                let rs = RunStyle::from_rpr(rpr, ns);
                match &mut s.character_style {
                    None => s.character_style = Some(rs),
                    Some(existing) => existing.update(&rs),
                }
            }
        }

        if matches!(s.style_type.as_deref(), Some("numbering" | "paragraph")) {
            let mut link = None;
            for ppr in ns.children(elem, &["w:pPr"]) {
                for num_pr in ns.children(ppr, &["w:numPr"]) {
                    if let Some(num_id) = last_child_with_val(num_pr, ns, "w:numId") {
                        link = ns.get(num_id, "w:val").map(str::to_string);
                    }
                }
            }
            s.numbering_style_link = link;
        }

        s
    }

    /// Fills every unset (`None`) sub-style from `parent`'s.
    ///
    /// Port of the Python `Style.resolve_based_on`.
    pub fn resolve_based_on(&mut self, parent: &Style) {
        if let Some(parent_table) = &parent.table_style {
            let ts = self.table_style.get_or_insert_with(TableStyle::new);
            ts.resolve_based_on(parent_table);
        }
        if let Some(parent_para) = &parent.paragraph_style {
            let ps = self.paragraph_style.get_or_insert_with(ParagraphStyle::new);
            ps.resolve_based_on(parent_para);
        }
        if let Some(parent_char) = &parent.character_style {
            let rs = self.character_style.get_or_insert_with(RunStyle::new);
            rs.resolve_based_on(parent_char);
        }
    }
}

/// The collection of every named style in a document, plus the
/// `docDefaults -> named style -> direct formatting` cascade that
/// resolves a `w:p`/`w:r`'s full, final [`ParagraphStyle`]/[`RunStyle`].
/// See the module docs for what's deferred.
///
/// Port of the Python `Styles`.
#[derive(Debug, Clone)]
pub struct Styles<'a, 'i> {
    /// Insertion-ordered (document order), matching Python's
    /// `OrderedDict` -- `generate_classes`/CSS-class numbering depend
    /// on a stable, deterministic iteration order.
    pub id_map: IndexMap<String, Style>,
    para_cache: HashMap<Node<'a, 'i>, ParagraphStyle>,
    para_char_cache: HashMap<Node<'a, 'i>, RunStyle>,
    run_cache: HashMap<Node<'a, 'i>, RunStyle>,
    classes: HashMap<BTreeMap<String, String>, (String, Css)>,
    counter: HashMap<String, u32>,
    default_styles: HashMap<String, Style>,
    tables: Tables<'a, 'i>,
    numbering_style_links: HashMap<String, String>,
    default_paragraph_style: Option<ParagraphStyle>,
    default_character_style: Option<RunStyle>,
    numbering: Option<Numbering>,
    /// See the module docs' "`calibre_num_id`: a tracked map" section.
    pub calibre_num_ids: HashMap<Node<'a, 'i>, (i32, String)>,
    /// The document-wide default font family/size/color, promoted
    /// from the most common paragraph-level value by `to_html.rs`'s
    /// [`cascade`](super::to_html::cascade). Consumed by the
    /// not-yet-ported `Styles::generate_css` for the `body { ... }`
    /// CSS rule. Set to the same values `Styles.cascade` starts from
    /// in Python (`self.body_font_family = 'serif'`, etc.) so a
    /// `Styles` that never runs `cascade` still has sensible
    /// defaults.
    pub body_font_family: String,
    pub body_font_size: String,
    pub body_color: String,
}

impl<'a, 'i> Styles<'a, 'i> {
    /// Port of `Styles(namespace, tables)`.
    pub fn new(tables: Tables<'a, 'i>) -> Self {
        Styles {
            id_map: IndexMap::new(),
            para_cache: HashMap::new(),
            para_char_cache: HashMap::new(),
            run_cache: HashMap::new(),
            classes: HashMap::new(),
            counter: HashMap::new(),
            default_styles: HashMap::new(),
            tables,
            numbering_style_links: HashMap::new(),
            default_paragraph_style: None,
            default_character_style: None,
            numbering: None,
            calibre_num_ids: HashMap::new(),
            body_font_family: "serif".to_string(),
            body_font_size: "10pt".to_string(),
            body_color: "currentColor".to_string(),
        }
    }

    /// Reads every `w:style` definition and `w:docDefaults`, then
    /// resolves each style's `w:basedOn` inheritance chain.
    ///
    /// Port of the Python `Styles.__call__` (minus taking `fonts`,
    /// unneeded here -- see the module docs).
    pub fn call(&mut self, root: Option<Node<'a, 'i>>, ns: &DocxNamespace) {
        self.default_paragraph_style = None;
        self.default_character_style = None;

        if let Some(root) = root {
            for s in ns.descendants(root, &["w:style"]) {
                let style = Style::from_elem(s, ns);
                if let Some(id) = style.style_id.clone() {
                    self.id_map.insert(id, style.clone());
                }
                if style.is_default {
                    if let Some(t) = style.style_type.clone() {
                        self.default_styles.insert(t, style.clone());
                    }
                }
                if let Some(link) = style.numbering_style_link.clone() {
                    if let Some(id) = &style.style_id {
                        self.numbering_style_links.insert(id.clone(), link);
                    }
                }
            }

            for dd in ns.children(root, &["w:docDefaults"]) {
                for pd in ns.children(dd, &["w:pPrDefault"]) {
                    for ppr in ns.children(pd, &["w:pPr"]) {
                        let ps = ParagraphStyle::from_ppr(ppr, ns);
                        match &mut self.default_paragraph_style {
                            None => self.default_paragraph_style = Some(ps),
                            Some(existing) => existing.update(&ps),
                        }
                    }
                }
                for pd in ns.children(dd, &["w:rPrDefault"]) {
                    for rpr in ns.children(pd, &["w:rPr"]) {
                        let rs = RunStyle::from_rpr(rpr, ns);
                        match &mut self.default_character_style {
                            None => self.default_character_style = Some(rs),
                            Some(existing) => existing.update(&rs),
                        }
                    }
                }
            }
        }

        let ids: Vec<String> = self.id_map.keys().cloned().collect();
        for id in ids {
            self.resolve_style_chain(&id);
        }
    }

    /// Port of the nested Python `resolve`/its driving loop.
    fn resolve_style_chain(&mut self, id: &str) {
        let already_resolved = self.id_map.get(id).map(|s| s.resolved).unwrap_or(true);
        if already_resolved {
            return;
        }
        let based_on = self.id_map.get(id).and_then(|s| s.based_on.clone());
        if let Some(parent_id) = &based_on {
            if self.id_map.contains_key(parent_id) {
                self.resolve_style_chain(parent_id);
                let parent = self.id_map.get(parent_id).cloned().unwrap();
                if let Some(child) = self.id_map.get_mut(id) {
                    child.resolve_based_on(&parent);
                }
            }
        }
        if let Some(s) = self.id_map.get_mut(id) {
            s.resolved = true;
        }
    }

    /// Whether a block/run style carries real (non-inherit) numbering.
    fn has_numbering(s: &ParagraphStyle) -> bool {
        s.numbering_id.is_some() && s.numbering_level.is_some()
    }

    /// The final, fully cascaded style for one `w:p`, cached by node
    /// identity. See the module docs for `calibre_num_ids`.
    ///
    /// Port of the Python `Styles.resolve_paragraph`.
    pub fn resolve_paragraph(&mut self, p: Node<'a, 'i>, ns: &DocxNamespace) -> ParagraphStyle {
        if let Some(cached) = self.para_cache.get(&p) {
            return cached.clone();
        }

        let mut direct_formatting: Option<ParagraphStyle> = None;
        let mut is_section_break = false;
        for ppr in ns.children(p, &["w:pPr"]) {
            let ps = ParagraphStyle::from_ppr(ppr, ns);
            match &mut direct_formatting {
                None => direct_formatting = Some(ps),
                Some(existing) => existing.update(&ps),
            }
            if !ns.children(ppr, &["w:sectPr"]).is_empty() {
                is_section_break = true;
            }
        }
        let direct_formatting = direct_formatting.unwrap_or_default();

        let mut parent_styles: Vec<ParagraphStyle> = Vec::new();
        if let Some(dps) = &self.default_paragraph_style {
            parent_styles.push(dps.clone());
        }
        if let Some(ts) = self.tables.para_style(p) {
            parent_styles.push(ts.clone());
        }

        let default_para = self.default_styles.get("paragraph").cloned();
        let mut linked_style: Option<Style> = None;
        let mut style_name: Option<String> = None;
        if let Some(link) = &direct_formatting.linked_style {
            if let Some(ls) = self.id_map.get(link).cloned() {
                style_name = ls.name.clone();
                if let Some(ps) = &ls.paragraph_style {
                    parent_styles.push(ps.clone());
                }
                if let Some(cs) = &ls.character_style {
                    self.para_char_cache.insert(p, cs.clone());
                }
                linked_style = Some(ls);
            }
        } else if let Some(dp) = &default_para {
            if let Some(ps) = &dp.paragraph_style {
                parent_styles.push(ps.clone());
            }
            if let Some(cs) = &dp.character_style {
                self.para_char_cache.insert(p, cs.clone());
            }
        }

        let mut is_numbering = Self::has_numbering(&direct_formatting);
        is_section_break = is_section_break && ns.children(p, &["w:r"]).is_empty();

        if is_numbering && !is_section_break {
            if let (Some(num_id), Some(lvl)) = (
                &direct_formatting.numbering_id,
                direct_formatting.numbering_level,
            ) {
                self.calibre_num_ids.insert(p, (lvl, num_id.clone()));
                if let Some(numbering) = &self.numbering {
                    if let Some(ps) = numbering.get_para_style(num_id, lvl as i64) {
                        parent_styles.push(ps.clone());
                    }
                }
            }
        }
        if !is_numbering && !is_section_break {
            if let Some(ls) = &linked_style {
                if let Some(lps) = &ls.paragraph_style {
                    if Self::has_numbering(lps) {
                        let num_id = lps.numbering_id.clone().unwrap();
                        let lvl = lps.numbering_level.unwrap();
                        self.calibre_num_ids.insert(p, (lvl, num_id.clone()));
                        is_numbering = true;
                        if let Some(numbering) = &self.numbering {
                            if let Some(ps) = numbering.get_para_style(&num_id, lvl as i64) {
                                parent_styles.push(ps.clone());
                            }
                        }
                    }
                }
            }
        }

        // Chaining `resolve_based_on` over `parent_styles` in reverse
        // (last-appended = highest priority, matching the Python
        // `para_val`'s `for ps in reversed(parent_styles)`) reproduces
        // "direct formatting wins, else the first non-inherit parent"
        // for every field at once, since `resolve_based_on` only ever
        // fills gaps that are still unset.
        let mut ans = direct_formatting.clone();
        for ps in parent_styles.iter().rev() {
            ans.resolve_based_on(ps);
        }
        if is_numbering {
            // Python skips `text_indent` entirely out of the per-attr
            // cascade when numbering applies (list indentation is
            // handled elsewhere), leaving it at its all-inherit
            // construction default regardless of direct formatting.
            ans.text_indent = None;
        }
        ans.linked_style = direct_formatting.linked_style.clone();
        ans.style_name = style_name;

        self.para_cache.insert(p, ans.clone());
        ans
    }

    /// Write-back setter for a cached paragraph's `text_indent`, the
    /// same post-`resolve_paragraph` mutation need
    /// [`Styles::set_run_font_family`]/[`Styles::clear_run_border`]
    /// cover for runs -- `to_html.py`'s tab-to-text-indent cleanup
    /// pass calls `resolve(wp)` and then mutates the object it got
    /// back in place.
    pub fn set_paragraph_text_indent(&mut self, p: Node<'a, 'i>, text_indent: String) {
        if let Some(cached) = self.para_cache.get_mut(&p) {
            cached.text_indent = Some(text_indent);
        }
    }

    /// Overwrites a cached paragraph's entire resolved style -- the
    /// same write-back need as [`Styles::set_paragraph_text_indent`],
    /// but for `mark_block_runs`, which mutates a resolved
    /// [`ParagraphStyle`]'s borders/margins/padding across half a
    /// dozen fields at once (merging a run of bordered paragraphs into
    /// one visual block) rather than touching a single field.
    pub fn set_paragraph_style(&mut self, p: Node<'a, 'i>, style: ParagraphStyle) {
        self.para_cache.insert(p, style);
    }

    /// The final, fully cascaded style for one `w:r`, cached by node
    /// identity. Font resolution stops short of `fonts.py`'s system
    /// font matching -- see the module docs.
    ///
    /// Port of the Python `Styles.resolve_run`.
    pub fn resolve_run(&mut self, r: Node<'a, 'i>, theme: &Theme, ns: &DocxNamespace) -> RunStyle {
        if let Some(cached) = self.run_cache.get(&r) {
            return cached.clone();
        }

        let p = ns.ancestor(r, "w:p");

        let mut direct_formatting: Option<RunStyle> = None;
        for rpr in ns.children(r, &["w:rPr"]) {
            let rs = RunStyle::from_rpr(rpr, ns);
            match &mut direct_formatting {
                None => direct_formatting = Some(rs),
                Some(existing) => existing.update(&rs),
            }
        }
        let direct_formatting = direct_formatting.unwrap_or_default();

        let mut parent_styles: Vec<RunStyle> = Vec::new();
        let default_char_index = if let Some(dcs) = &self.default_character_style {
            parent_styles.push(dcs.clone());
            Some(0usize)
        } else {
            None
        };

        if let Some(p) = p {
            if let Some(pstyle) = self.para_char_cache.get(&p) {
                parent_styles.push(pstyle.clone());
            }
        }

        // Table overrides applied before paragraph overrides would
        // match the spec more closely, but Word does it this way too
        // (see the December 2007 table header in calibre's demo
        // document, per the Python comment this reproduces).
        if let Some(p) = p {
            if let Some(ts) = self.tables.run_style(p) {
                parent_styles.push(ts.clone());
            }
        }

        if let Some(link) = &direct_formatting.linked_style {
            if let Some(cs) = self
                .id_map
                .get(link)
                .and_then(|s| s.character_style.clone())
            {
                parent_styles.push(cs);
            }
        } else if let Some(dc) = self.default_styles.get("character") {
            if let Some(cs) = &dc.character_style {
                parent_styles.push(cs.clone());
            }
        }

        let mut ans = direct_formatting.clone();
        for ps in parent_styles.iter().rev() {
            ans.resolve_based_on(ps);
        }

        // Toggle properties (ECMA-376 §17.7.3's XOR rule) override
        // whatever the plain cascade above produced for them, but
        // only when direct formatting itself left the property unset
        // -- direct formatting always wins outright, toggle or not.
        macro_rules! toggle {
            ($field:ident) => {
                if direct_formatting.$field.is_none() {
                    let votes: Vec<bool> = parent_styles
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| Some(*i) != default_char_index)
                        .filter_map(|(_, rs)| rs.$field)
                        .collect();
                    ans.$field = Some(if !votes.is_empty() {
                        votes.iter().filter(|&&v| v).count() % 2 == 1
                    } else if let Some(dcs) = &self.default_character_style {
                        dcs.$field == Some(true)
                    } else {
                        false
                    });
                }
            };
        }
        toggle!(b);
        toggle!(b_cs);
        toggle!(caps);
        toggle!(emboss);
        toggle!(i);
        toggle!(i_cs);
        toggle!(imprint);
        toggle!(shadow);
        toggle!(small_caps);
        toggle!(strike);
        toggle!(vanish);

        if let Some(ff) = &ans.font_family {
            // Python continues: `self.fonts.family_for(ff, ans.b,
            // ans.i)`, matching against the system's installed fonts.
            // Not ported (see the module docs) -- `font_family` stops
            // at the theme-resolved literal name.
            ans.font_family = Some(theme.resolve_font_family(ff));
        }

        self.run_cache.insert(r, ans.clone());
        ans
    }

    /// Overwrites a cached run's `font_family`, persisting the change
    /// the way Python's `resolve_run` callers do when they mutate the
    /// object `resolve_run` returned in place (e.g. `to_html.py`'s
    /// `convert_run`, after remapping a symbol font's text, sets
    /// `style.font_family = 'sans-serif'` on the very `RunStyle`
    /// `resolve_run` cached -- a real, persisted mutation later CSS
    /// generation observes, not a throwaway local). `resolve_run`
    /// returns an owned clone here, so that mutation needs an explicit
    /// write-back; call this only after `resolve_run(r, ...)` has
    /// already populated the cache for `r`.
    pub fn set_run_font_family(&mut self, r: Node<'a, 'i>, font_family: String) {
        if let Some(cached) = self.run_cache.get_mut(&r) {
            cached.font_family = Some(font_family);
        }
    }

    /// Clears a cached run's border, the same write-back need as
    /// [`Styles::set_run_font_family`] but for `to_html.py`'s
    /// `convert_p`, which moves a run of same-bordered spans' border
    /// onto a single wrapping element and calls
    /// `style.clear_border_css()` on each original run so its own
    /// (now redundant) border CSS isn't also emitted.
    pub fn clear_run_border(&mut self, r: Node<'a, 'i>) {
        if let Some(cached) = self.run_cache.get_mut(&r) {
            cached.clear_border_css();
        }
    }

    /// Overwrites a cached run's entire resolved style -- the
    /// [`RunStyle`] counterpart to [`Styles::set_paragraph_style`],
    /// for `to_html.rs`'s [`cascade`](super::to_html::cascade), which
    /// mutates several `RunStyle` fields per run at once (hoisting
    /// properties shared by every run in a paragraph up onto the
    /// paragraph itself).
    pub fn set_run_style(&mut self, r: Node<'a, 'i>, style: RunStyle) {
        self.run_cache.insert(r, style);
    }

    /// Registers a `w:tbl` with this instance's [`Tables`].
    ///
    /// `to_html.py`'s `Convert.tables` and `Convert.styles` are two
    /// separate attributes referring to the same object -- Python
    /// mutates `self.tables` directly (e.g. in `read_page_properties`)
    /// and the change is visible through `self.styles.tables` too.
    /// Here [`Styles::new`] takes ownership of the one `Tables`
    /// instance instead, so callers outside this module go through
    /// this method rather than holding their own handle to it.
    pub fn register_table(&mut self, tbl: Node<'a, 'i>, ns: &DocxNamespace) {
        self.tables.register(tbl, &self.id_map, ns);
    }

    /// Discards a named paragraph style's own `w:numPr` level in
    /// favour of the level `numbering.xml` actually links to that
    /// style (Word ignores a level set directly inside a style
    /// definition).
    ///
    /// Port of the Python `Styles.resolve_numbering`.
    pub fn resolve_numbering(&mut self, numbering: Numbering) {
        let ids: Vec<String> = self.id_map.keys().cloned().collect();
        for id in &ids {
            let numbering_id = match self.id_map.get(id).and_then(|s| s.paragraph_style.as_ref()) {
                Some(ps) if ps.numbering_id.is_some() => ps.numbering_id.clone().unwrap(),
                _ => continue,
            };
            let lvl = numbering.get_pstyle(&numbering_id, id);
            if let Some(style) = self.id_map.get_mut(id) {
                if let Some(ps) = &mut style.paragraph_style {
                    match lvl {
                        None => {
                            ps.numbering_id = None;
                            ps.numbering_level = None;
                        }
                        Some(l) => ps.numbering_level = Some(l as i32),
                    }
                }
            }
        }
        self.numbering = Some(numbering);
    }

    /// Zeroes the margin between two paragraphs sharing the same
    /// linked style when both request contextual spacing.
    ///
    /// Port of the Python `Styles.apply_contextual_spacing`.
    pub fn apply_contextual_spacing(&mut self, paras: &[Node<'a, 'i>], ns: &DocxNamespace) {
        let mut last_para: Option<Node<'a, 'i>> = None;
        for &p in paras {
            if let Some(last) = last_para {
                let ls = self.resolve_paragraph(last, ns);
                let ps = self.resolve_paragraph(p, ns);
                if ls.linked_style.is_some() && ls.linked_style == ps.linked_style {
                    if ls.contextual_spacing == Some(true) {
                        if let Some(cached) = self.para_cache.get_mut(&last) {
                            cached.margin_bottom = Some("0".to_string());
                        }
                    }
                    if ps.contextual_spacing == Some(true) {
                        if let Some(cached) = self.para_cache.get_mut(&p) {
                            cached.margin_top = Some("0".to_string());
                        }
                    }
                }
            }
            last_para = Some(p);
        }
    }

    /// Port of the Python `Styles.apply_section_page_breaks`.
    pub fn apply_section_page_breaks(&mut self, paras: &[Node<'a, 'i>], ns: &DocxNamespace) {
        for &p in paras {
            self.resolve_paragraph(p, ns);
            if let Some(cached) = self.para_cache.get_mut(&p) {
                cached.page_break_before = Some(true);
            }
        }
    }

    /// Interns one CSS declaration set under a `{prefix}_{N}` class
    /// name, returning the (possibly pre-existing) name.
    ///
    /// Port of the Python `Styles.register`.
    pub fn register(&mut self, css: Css, prefix: &str) -> String {
        let key: BTreeMap<String, String> =
            css.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Some((name, _)) = self.classes.get(&key) {
            return name.clone();
        }
        let count = self.counter.entry(prefix.to_string()).or_insert(0);
        *count += 1;
        let name = format!("{prefix}_{count}");
        self.classes.insert(key, (name.clone(), css));
        name
    }

    /// Port of the Python `Styles.class_name`.
    pub fn class_name(&self, css: &Css) -> Option<&str> {
        let key: BTreeMap<String, String> =
            css.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        self.classes.get(&key).map(|(name, _)| name.as_str())
    }

    /// Registers a CSS class for every cached paragraph/run style with
    /// non-empty CSS.
    ///
    /// Port of the Python `Styles.generate_classes`.
    pub fn generate_classes(&mut self) {
        let para_styles: Vec<ParagraphStyle> = self.para_cache.values().cloned().collect();
        for bs in para_styles {
            let css = bs.css();
            if !css.is_empty() {
                self.register(css, "block");
            }
        }
        let run_styles: Vec<RunStyle> = self.run_cache.values().cloned().collect();
        for bs in run_styles {
            let css = bs.css();
            if !css.is_empty() {
                self.register(css, "text");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const DOC_OPEN: &str =
        r#"<w:style xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

    fn style_of(style_type: &str, body: &str) -> Style {
        let xml: &'static str = Box::leak(
            format!(r#"{DOC_OPEN}<w:style w:type="{style_type}" w:styleId="S1">{body}</w:style></w:style>"#)
                .into_boxed_str(),
        );
        let doc = Document::parse(xml).expect("valid XML");
        let ns = DocxNamespace::default();
        let inner = ns.first_child(doc.root_element(), "w:style").unwrap();
        Style::from_elem(inner, &ns)
    }

    #[test]
    fn page_properties_default_to_a4() {
        let p = PageProperties::new();
        assert_eq!(p.width, 595.28);
        assert_eq!(p.margin_left, 72.0);
    }

    #[test]
    fn page_properties_reads_size_and_margins() {
        let (doc, ns) = {
            let xml: &'static str = Box::leak(
                r#"<w:sectPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
                    <w:pgSz w:w="12240" w:h="15840"/>
                    <w:pgMar w:left="1440" w:right="1440"/>
                </w:sectPr>"#
                    .to_string()
                    .into_boxed_str(),
            );
            (
                Document::parse(xml).expect("valid XML"),
                DocxNamespace::default(),
            )
        };
        let p = PageProperties::from_sect_prs(&[doc.root_element()], &ns);
        assert_eq!(p.width, 612.0);
        assert_eq!(p.height, 792.0);
        assert_eq!(p.margin_left, 72.0);
        assert_eq!(p.margin_right, 72.0);
    }

    #[test]
    fn style_reads_name_and_based_on() {
        let s = style_of(
            "paragraph",
            r#"<w:name w:val="Heading 1"/><w:basedOn w:val="Normal"/>"#,
        );
        assert_eq!(s.name.as_deref(), Some("Heading 1"));
        assert_eq!(s.based_on.as_deref(), Some("Normal"));
        assert_eq!(s.style_id.as_deref(), Some("S1"));
    }

    #[test]
    fn numbering_style_never_has_a_based_on() {
        let s = style_of("numbering", r#"<w:basedOn w:val="Whatever"/>"#);
        assert_eq!(s.based_on, None);
    }

    #[test]
    fn table_style_type_builds_a_table_style_and_paragraph_style() {
        let s = style_of(
            "table",
            r#"<w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:pPr><w:jc w:val="center"/></w:pPr>"#,
        );
        assert!(s.table_style.is_some());
        assert_eq!(s.table_style.unwrap().width.as_deref(), Some("100%"));
        assert!(s.paragraph_style.is_some());
    }

    #[test]
    fn paragraph_style_numbering_link_reads_num_id() {
        let s = style_of(
            "paragraph",
            r#"<w:pPr><w:numPr><w:numId w:val="3"/></w:numPr></w:pPr>"#,
        );
        assert_eq!(s.numbering_style_link.as_deref(), Some("3"));
    }

    #[test]
    fn character_style_never_reads_a_numbering_link() {
        let s = style_of("character", r#"<w:rPr><w:b/></w:rPr>"#);
        assert_eq!(s.numbering_style_link, None);
    }

    #[test]
    fn resolve_based_on_creates_missing_sub_styles_from_the_parent() {
        let mut parent = Style::default();
        parent.table_style = Some({
            let mut ts = TableStyle::new();
            ts.width = Some("10pt".to_string());
            ts
        });
        let mut child = Style::default();
        child.resolve_based_on(&parent);
        assert_eq!(
            child.table_style.unwrap().width.as_deref(),
            Some("10pt"),
            "child had no table_style of its own, so it inherits the parent's wholesale"
        );
    }

    const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn parse_doc(root_tag: &str, body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<{root_tag} {W_NS}>{body}</{root_tag}>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    #[test]
    fn call_resolves_the_based_on_chain_and_doc_defaults() {
        let (doc, ns) = parse_doc(
            "w:styles",
            r#"<w:docDefaults>
                <w:rPrDefault><w:rPr><w:sz w:val="20"/></w:rPr></w:rPrDefault>
               </w:docDefaults>
               <w:style w:type="paragraph" w:styleId="Normal">
                <w:pPr><w:spacing w:after="240"/></w:pPr>
               </w:style>
               <w:style w:type="paragraph" w:styleId="Heading1">
                <w:basedOn w:val="Normal"/>
                <w:pPr><w:jc w:val="center"/></w:pPr>
               </w:style>"#,
        );
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(doc.root_element()), &ns);

        let heading = &styles.id_map["Heading1"];
        assert_eq!(
            heading
                .paragraph_style
                .as_ref()
                .unwrap()
                .text_align
                .as_deref(),
            Some("center"),
            "Heading1's own direct formatting"
        );
        assert_eq!(
            heading
                .paragraph_style
                .as_ref()
                .unwrap()
                .margin_bottom
                .as_deref(),
            Some("12pt"),
            "inherited from Normal via basedOn"
        );
        assert_eq!(
            styles.default_character_style.as_ref().unwrap().font_size,
            Some(10.0),
            "rPrDefault read into default_character_style"
        );
    }

    #[test]
    fn resolve_paragraph_direct_formatting_beats_the_linked_style() {
        let (doc, ns) = parse_doc(
            "w:root",
            r#"<w:styles>
                <w:style w:type="paragraph" w:styleId="Centered">
                 <w:pPr><w:jc w:val="center"/></w:pPr>
                </w:style>
               </w:styles>
               <w:body>
                <w:p><w:pPr><w:pStyle w:val="Centered"/><w:jc w:val="right"/></w:pPr></w:p>
               </w:body>"#,
        );
        let styles_root = ns.first_child(doc.root_element(), "w:styles").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(styles_root), &ns);

        let body = ns.first_child(doc.root_element(), "w:body").unwrap();
        let p = ns.first_child(body, "w:p").unwrap();
        let resolved = styles.resolve_paragraph(p, &ns);
        assert_eq!(
            resolved.text_align.as_deref(),
            Some("right"),
            "direct formatting overrides the linked style"
        );
        assert_eq!(resolved.style_name, None, "Centered has no w:name");
    }

    #[test]
    fn resolve_paragraph_falls_back_to_the_default_paragraph_style() {
        let (doc, ns) = parse_doc(
            "w:root",
            r#"<w:styles>
                <w:docDefaults><w:pPrDefault><w:pPr>
                 <w:ind w:left="720"/>
                </w:pPr></w:pPrDefault></w:docDefaults>
               </w:styles>
               <w:body><w:p/></w:body>"#,
        );
        let styles_root = ns.first_child(doc.root_element(), "w:styles").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(styles_root), &ns);

        let body = ns.first_child(doc.root_element(), "w:body").unwrap();
        let p = ns.first_child(body, "w:p").unwrap();
        let resolved = styles.resolve_paragraph(p, &ns);
        assert_eq!(resolved.margin_left.as_deref(), Some("36pt"));
    }

    #[test]
    fn resolve_paragraph_caches_by_node_identity() {
        let (doc, ns) = parse_doc("w:body", "<w:p/>");
        let p = ns.first_child(doc.root_element(), "w:p").unwrap();
        let mut styles = Styles::new(Tables::default());
        let first = styles.resolve_paragraph(p, &ns);
        let second = styles.resolve_paragraph(p, &ns);
        assert_eq!(first, second);
        assert_eq!(styles.para_cache.len(), 1);
    }

    #[test]
    fn numbering_skips_text_indent_and_records_calibre_num_id() {
        let (doc, ns) = parse_doc(
            "w:body",
            r#"<w:p><w:pPr>
                <w:ind w:firstLine="240"/>
                <w:numPr><w:ilvl w:val="1"/><w:numId w:val="9"/></w:numPr>
               </w:pPr></w:p>"#,
        );
        let p = ns.first_child(doc.root_element(), "w:p").unwrap();
        let mut styles = Styles::new(Tables::default());
        let resolved = styles.resolve_paragraph(p, &ns);
        assert_eq!(
            resolved.text_indent, None,
            "text-indent is dropped entirely once numbering applies"
        );
        assert_eq!(styles.calibre_num_ids.get(&p), Some(&(1, "9".to_string())));
    }

    #[test]
    fn a_section_break_paragraph_with_no_runs_is_not_treated_as_numbered() {
        let (doc, ns) = parse_doc(
            "w:body",
            r#"<w:p><w:pPr>
                <w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>
                <w:sectPr/>
               </w:pPr></w:p>"#,
        );
        let p = ns.first_child(doc.root_element(), "w:p").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.resolve_paragraph(p, &ns);
        assert!(
            styles.calibre_num_ids.is_empty(),
            "a section-break paragraph with no w:r children is excluded from numbering"
        );
    }

    #[test]
    fn resolve_run_direct_formatting_beats_the_toggle_vote() {
        let (doc, ns) = parse_doc(
            "w:body",
            r#"<w:p><w:r><w:rPr><w:b w:val="0"/></w:rPr></w:r></w:p>"#,
        );
        let body = doc.root_element();
        let p = ns.first_child(body, "w:p").unwrap();
        let r = ns.first_child(p, "w:r").unwrap();
        let mut styles = Styles::new(Tables::default());
        let mut default_char = RunStyle::new();
        default_char.b = Some(true);
        styles.default_character_style = Some(default_char);
        let theme = Theme::default();
        let resolved = styles.resolve_run(r, &theme, &ns);
        assert_eq!(
            resolved.b,
            Some(false),
            "explicit w:b val=0 wins outright, no XOR vote involved"
        );
    }

    #[test]
    fn resolve_run_toggle_vote_is_a_parity_check_across_parent_styles() {
        let (doc, ns) = parse_doc(
            "w:root",
            r#"<w:styles>
                <w:style w:type="paragraph" w:styleId="P1">
                 <w:rPr><w:b/></w:rPr>
                </w:style>
                <w:style w:type="character" w:styleId="C1">
                 <w:rPr><w:b/></w:rPr>
                </w:style>
               </w:styles>
               <w:body>
                <w:p><w:pPr><w:pStyle w:val="P1"/></w:pPr>
                 <w:r><w:rPr><w:rStyle w:val="C1"/></w:rPr></w:r>
                </w:p>
               </w:body>"#,
        );
        let styles_root = ns.first_child(doc.root_element(), "w:styles").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(styles_root), &ns);
        let theme = Theme::default();

        let body = ns.first_child(doc.root_element(), "w:body").unwrap();
        let p = ns.first_child(body, "w:p").unwrap();
        let r = ns.first_child(p, "w:r").unwrap();

        // Populates para_char_cache from P1's own character_style.
        styles.resolve_paragraph(p, &ns);
        let resolved = styles.resolve_run(r, &theme, &ns);
        assert_eq!(
            resolved.b,
            Some(false),
            "two \"on\" votes (P1's linked char style and C1) cancel out"
        );
    }

    #[test]
    fn resolve_run_theme_resolves_font_family_but_does_not_match_system_fonts() {
        let (doc, ns) = parse_doc(
            "w:body",
            r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Arial"/></w:rPr></w:r></w:p>"#,
        );
        let p = ns.first_child(doc.root_element(), "w:p").unwrap();
        let r = ns.first_child(p, "w:r").unwrap();
        let mut styles = Styles::new(Tables::default());
        let theme = Theme::default();
        let resolved = styles.resolve_run(r, &theme, &ns);
        assert_eq!(resolved.font_family.as_deref(), Some("Arial"));
    }

    #[test]
    fn apply_contextual_spacing_zeroes_the_shared_margin() {
        let (doc, ns) = parse_doc(
            "w:root",
            r#"<w:styles>
                <w:style w:type="paragraph" w:styleId="Body">
                 <w:pPr><w:contextualSpacing/></w:pPr>
                </w:style>
               </w:styles>
               <w:body>
                <w:p><w:pPr><w:pStyle w:val="Body"/></w:pPr></w:p>
                <w:p><w:pPr><w:pStyle w:val="Body"/></w:pPr></w:p>
               </w:body>"#,
        );
        let styles_root = ns.first_child(doc.root_element(), "w:styles").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(styles_root), &ns);
        let body = ns.first_child(doc.root_element(), "w:body").unwrap();
        let paras: Vec<Node> = ns.children(body, &["w:p"]);
        assert_eq!(paras.len(), 2);

        styles.apply_contextual_spacing(&paras, &ns);
        let first = styles.resolve_paragraph(paras[0], &ns);
        let second = styles.resolve_paragraph(paras[1], &ns);
        assert_eq!(first.margin_bottom.as_deref(), Some("0"));
        assert_eq!(second.margin_top.as_deref(), Some("0"));
    }

    #[test]
    fn apply_section_page_breaks_marks_every_paragraph() {
        let (doc, ns) = parse_doc("w:body", "<w:p/><w:p/>");
        let paras: Vec<Node> = ns.children(doc.root_element(), &["w:p"]);
        let mut styles = Styles::new(Tables::default());
        styles.apply_section_page_breaks(&paras, &ns);
        for &p in &paras {
            assert_eq!(
                styles.resolve_paragraph(p, &ns).page_break_before,
                Some(true)
            );
        }
    }

    #[test]
    fn resolve_numbering_discards_a_styles_own_level_in_favour_of_the_link() {
        let (doc, ns) = parse_doc(
            "w:styles",
            r#"<w:style w:type="paragraph" w:styleId="ListPara">
                <w:pPr><w:numPr><w:ilvl w:val="5"/><w:numId w:val="3"/></w:numPr></w:pPr>
               </w:style>"#,
        );
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(doc.root_element()), &ns);

        let mut numbering = Numbering::new();
        numbering.instances.insert(
            "3".to_string(),
            super::super::numbering::NumberingDefinition::new(None),
        );
        let mut level = super::super::numbering::Level::new();
        level.para_link = Some("ListPara".to_string());
        numbering
            .instances
            .get_mut("3")
            .unwrap()
            .levels
            .insert(2, level);

        styles.resolve_numbering(numbering);
        let ps = styles.id_map["ListPara"].paragraph_style.as_ref().unwrap();
        assert_eq!(
            ps.numbering_level,
            Some(2),
            "the style's own w:ilvl (5) is discarded in favour of numbering.xml's link (level 2)"
        );
    }

    #[test]
    fn resolve_numbering_clears_a_style_that_numbering_xml_never_links_back() {
        let (doc, ns) = parse_doc(
            "w:styles",
            r#"<w:style w:type="paragraph" w:styleId="Orphan">
                <w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="7"/></w:numPr></w:pPr>
               </w:style>"#,
        );
        let mut styles = Styles::new(Tables::default());
        styles.call(Some(doc.root_element()), &ns);
        styles.resolve_numbering(Numbering::new());
        let ps = styles.id_map["Orphan"].paragraph_style.as_ref().unwrap();
        assert_eq!(ps.numbering_id, None);
        assert_eq!(ps.numbering_level, None);
    }

    #[test]
    fn register_dedupes_identical_css_and_class_name_looks_it_back_up() {
        let mut styles = Styles::new(Tables::default());
        let mut css = Css::new();
        css.insert("color".to_string(), "red".to_string());
        let a = styles.register(css.clone(), "block");
        let b = styles.register(css.clone(), "block");
        assert_eq!(a, b, "identical css reuses the same class");
        assert_eq!(styles.class_name(&css), Some(a.as_str()));

        let mut other = Css::new();
        other.insert("color".to_string(), "blue".to_string());
        let c = styles.register(other, "block");
        assert_ne!(a, c);
        assert_eq!(c, "block_2");
    }

    #[test]
    fn generate_classes_registers_every_non_empty_cached_style() {
        let (doc, ns) = parse_doc(
            "w:body",
            r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr></w:p>"#,
        );
        let p = ns.first_child(doc.root_element(), "w:p").unwrap();
        let mut styles = Styles::new(Tables::default());
        styles.resolve_paragraph(p, &ns);
        styles.generate_classes();
        assert_eq!(styles.classes.len(), 1);
    }
}
