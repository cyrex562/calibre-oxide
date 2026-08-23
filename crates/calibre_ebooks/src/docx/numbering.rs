//! List/numbering definitions: `w:numbering.xml`'s abstract and
//! concrete numbering instances, resolved into per-level formatting.
//!
//! Partial port of `old_src/src/calibre/ebooks/docx/numbering.py` --
//! reading definitions (`Level`, `NumberingDefinition`, `Numbering`)
//! plus [`Level::css`]/[`Level::char_css`]. `Numbering.apply_markup`
//! itself (which turns runs of numbered paragraphs into `<ol>`/`<ul>`
//! HTML) lives in `to_html.rs` as `apply_numbering_markup`, alongside
//! this module's other `Dom`-needing orchestration -- see `tables.rs`'s
//! module docs for the same pattern, and issue #130/#286.
//! [`Level::css`] always omits `list-style-image` (a `w:lvlPicBulletId`
//! picture bullet needs an `Images` type from the not-yet-ported
//! `images.py`, issue #289) -- see its own docs for why that's a safe
//! deferral, not a behavior gap.
//!
//! # A narrower `Numbering::call` signature than Python's
//!
//! Python's `Numbering.__call__(self, root, styles, rid_map)` takes
//! the whole `Styles` collection just to read one field,
//! `styles.numbering_style_links`. `Styles` (the paragraph/run
//! cascade orchestrator, `styles.py`) is itself deferred -- it needs
//! `Tables.para_style`/`run_style`, not yet ported either -- so this
//! takes that one map directly: `numbering_style_links: &HashMap<String, String>`.
//! Once `Styles` lands it can pass `&styles.numbering_style_links`
//! straight through; nothing here needs to change.
//!
//! # A reproduced quirk: `create_instance`'s start-override bug
//!
//! Python's nested `create_instance` builds `start_overrides` as
//! `{ilvl: start_override}` but then applies it with
//! `nd.levels[ilvl].start = start_override` -- reading the *closure*
//! variable `start_override` (whatever `w:startOverride` was read
//! last across the whole `n` element), not the dict's own per-`ilvl`
//! value the loop just unpacked. Every overridden level ends up with
//! the *same* (last-seen) start value rather than its own. Reproduced
//! as-is below (see [`create_instance`]) rather than fixed, matching
//! this module's sibling files' treatment of upstream quirks.

use std::collections::HashMap;

use regex::Regex;
use roxmltree::Node;

use super::block_styles::{Css, ParagraphStyle};
use super::char_styles::RunStyle;
use super::names::DocxNamespace;

const ROMAN_CODING: [(i64, &str); 13] = [
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

/// Port of `calibre.ebooks.metadata.roman`.
pub fn to_roman(num: i64) -> String {
    if num <= 0 || num >= 4000 {
        return num.to_string();
    }
    let mut n = num;
    let mut ans = String::new();
    for &(d, r) in &ROMAN_CODING {
        while n >= d {
            ans.push_str(r);
            n -= d;
        }
    }
    ans
}

/// Port of the Python `STYLE_MAP`.
fn style_map(val: &str) -> &'static str {
    match val {
        "aiueo" | "aiueoFullWidth" => "hiragana",
        "hebrew1" => "hebrew",
        "iroha" | "irohaFullWidth" => "katakana-iroha",
        "lowerLetter" => "lower-alpha",
        "lowerRoman" => "lower-roman",
        "none" => "none",
        "upperLetter" => "upper-alpha",
        "upperRoman" => "upper-roman",
        "chineseCounting" => "cjk-ideographic",
        "decimalZero" => "decimal-leading-zero",
        _ => "decimal",
    }
}

/// Port of the Python `alphabet`.
fn alphabet(val: i64, lower: bool) -> String {
    const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let letters = if lower { LOWER } else { UPPER };
    let idx = ((val - 1).unsigned_abs() as usize) % letters.len();
    (letters[idx] as char).to_string()
}

/// Port of the Python `alphabet_map`, applied to one counter value.
fn format_counter_value(fmt: &str, val: i64) -> String {
    match fmt {
        "lower-alpha" => alphabet(val, true),
        "upper-alpha" => alphabet(val, false),
        "lower-roman" => to_roman(val).to_lowercase(),
        "upper-roman" => to_roman(val),
        "decimal-leading-zero" => format!("0{val}"),
        _ => format!("{val}"),
    }
}

/// One `w:lvl`'s resolved formatting -- the numbering/bullet template,
/// linked paragraph/character styles, and restart behaviour.
///
/// Port of the Python `Level`.
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub restart: Option<i64>,
    pub start: i64,
    pub fmt: String,
    pub para_link: Option<String>,
    pub paragraph_style: Option<ParagraphStyle>,
    pub character_style: Option<RunStyle>,
    pub is_numbered: bool,
    pub num_template: Option<String>,
    pub bullet_template: Option<String>,
    pub pic_id: Option<String>,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            restart: None,
            start: 0,
            fmt: "decimal".to_string(),
            para_link: None,
            paragraph_style: None,
            character_style: None,
            is_numbered: false,
            num_template: None,
            bullet_template: None,
            pic_id: None,
        }
    }
}

impl Level {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `Level(namespace, lvl)`.
    pub fn from_lvl(lvl: Node, ns: &DocxNamespace) -> Self {
        let mut level = Self::new();
        level.read_from_xml(lvl, ns);
        level
    }

    /// Port of the Python `Level.read_from_xml`.
    pub fn read_from_xml(&mut self, lvl: Node, ns: &DocxNamespace) {
        for lr in ns.children(lvl, &["w:lvlRestart"]) {
            if let Some(v) = ns.get(lr, "w:val").and_then(|v| v.trim().parse().ok()) {
                self.restart = Some(v);
            }
        }
        for lr in ns.children(lvl, &["w:start"]) {
            if let Some(v) = ns.get(lr, "w:val").and_then(|v| v.trim().parse().ok()) {
                self.start = v;
            }
        }
        for rpr in ns.children(lvl, &["w:rPr"]) {
            let rs = RunStyle::from_rpr(rpr, ns);
            match &mut self.character_style {
                None => self.character_style = Some(rs),
                Some(cs) => cs.update(&rs),
            }
        }

        let mut lt: Option<String> = None;
        for lr in ns.children(lvl, &["w:lvlText"]) {
            lt = ns.get(lr, "w:val").map(str::to_string);
        }

        for lr in ns.children(lvl, &["w:numFmt"]) {
            let Some(val) = ns.get(lr, "w:val") else {
                continue;
            };
            if val == "bullet" {
                self.is_numbered = false;
                let is_symbol_font = self
                    .character_style
                    .as_ref()
                    .and_then(|cs| cs.font_family.as_deref())
                    .map(|f| matches!(f.to_lowercase().as_str(), "wingdings" | "symbol"))
                    .unwrap_or(false);
                if matches!(lt.as_deref(), Some("\u{f0a7}") | Some("o")) || is_symbol_font {
                    self.fmt = match lt.as_deref() {
                        Some("\u{f0a7}") => "square",
                        Some("o") => "circle",
                        _ => "disc",
                    }
                    .to_string();
                } else {
                    self.bullet_template.clone_from(&lt);
                }
                for lpid in ns.children(lvl, &["w:lvlPicBulletId"]) {
                    self.pic_id = ns.get(lpid, "w:val").map(str::to_string);
                }
            } else {
                self.is_numbered = true;
                self.fmt = style_map(val).to_string();
                if let Some(text) = lt.as_deref().filter(|t| !t.is_empty()) {
                    let only_percent_digits = Regex::new(r"^%\d+\.$").unwrap().is_match(text);
                    if !only_percent_digits {
                        self.num_template = Some(text.to_string());
                    }
                }
            }
        }

        for lr in ns.children(lvl, &["w:pStyle"]) {
            self.para_link = ns.get(lr, "w:val").map(str::to_string);
        }

        for ppr in ns.children(lvl, &["w:pPr"]) {
            let ps = ParagraphStyle::from_ppr(ppr, ns);
            match &mut self.paragraph_style {
                None => self.paragraph_style = Some(ps),
                Some(existing) => existing.update(&ps),
            }
        }
    }

    /// Renders a `%N` numbering template against the current counter
    /// state, e.g. `"%1.%2"` with counter `{0: 2, 1: 3}` at `ilvl == 1`
    /// yields `"1.3"` (the deeper level, `%2`, reads the counter as-is
    /// since `x == ilvl`; every shallower level, `%1`, reads one *less*
    /// than its counter, since that ancestor level already advanced
    /// past the value that was current when this item started)
    /// followed by a trailing non-breaking space.
    ///
    /// Port of the Python `Level.format_template`.
    pub fn format_template(
        &self,
        counter: &HashMap<i64, i64>,
        ilvl: i64,
        template: &str,
    ) -> String {
        let re = Regex::new(r"%(\d+)").unwrap();
        let substituted = re.replace_all(template, |caps: &regex::Captures| {
            let Ok(n) = caps[1].parse::<i64>() else {
                return String::new();
            };
            let x = n - 1;
            if x > ilvl {
                return String::new();
            }
            let Some(&count) = counter.get(&x) else {
                return String::new();
            };
            let val = count - if x == ilvl { 0 } else { 1 };
            format_counter_value(&self.fmt, val)
        });
        format!("{}\u{a0}", substituted.trim_end())
    }

    /// The CSS for this level's `<ol>`/`<ul>` wrapper -- always just
    /// `list-style-type`. Python's version also resolves a
    /// `w:lvlPicBulletId` picture bullet into a `list-style-image`
    /// URL via `images.generate_filename(rid, rid_map, ...)`, but
    /// swallows any resolution failure with `except Exception: fname
    /// = None` -- i.e. silently drops the image and falls back to
    /// exactly what this always does. Deferred until `images.py` is
    /// ported (issue #289); until then every level, picture-bullet or
    /// not, behaves like that already-common failure path.
    ///
    /// Port of the Python `Level.css`, minus the `images`/`pic_map`/
    /// `rid_map` picture-bullet branch.
    pub fn css(&self) -> Css {
        let mut c = Css::new();
        c.insert("list-style-type".to_string(), self.fmt.clone());
        c
    }

    /// This level's run-level CSS (for the rendered bullet/number
    /// text), minus `font-family` -- Word's numbering-run font is
    /// usually a symbol font inappropriate for the *rendered* Unicode
    /// bullet character.
    ///
    /// Port of the Python `Level.char_css`.
    pub fn char_css(&self) -> Css {
        let mut c = self
            .character_style
            .as_ref()
            .map(RunStyle::css)
            .unwrap_or_default();
        c.shift_remove("font-family");
        c
    }
}

/// One `w:abstractNum`'s per-level definitions.
///
/// Port of the Python `NumberingDefinition`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NumberingDefinition {
    pub levels: HashMap<i64, Level>,
    pub abstract_numbering_definition_id: Option<String>,
}

impl NumberingDefinition {
    pub fn new(an_id: Option<String>) -> Self {
        Self {
            levels: HashMap::new(),
            abstract_numbering_definition_id: an_id,
        }
    }

    /// Port of `NumberingDefinition(namespace, parent, an_id)`.
    pub fn from_parent(parent: Node, ns: &DocxNamespace, an_id: Option<String>) -> Self {
        let mut nd = Self::new(an_id);
        for lvl in ns.children(parent, &["w:lvl"]) {
            let ilvl = ns
                .get(lvl, "w:ilvl")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            nd.levels.insert(ilvl, Level::from_lvl(lvl, ns));
        }
        nd
    }
}

/// Applies a `w:num`'s `w:lvlOverride`s on top of a copy of its
/// abstract definition.
///
/// Port of the nested Python `create_instance` -- see the module docs
/// for the reproduced start-override quirk.
fn create_instance(
    ns: &DocxNamespace,
    n: Node,
    definition: &NumberingDefinition,
) -> NumberingDefinition {
    let mut nd = definition.clone();
    let mut start_override_ilvls: Vec<Option<i64>> = Vec::new();
    let mut last_start_override: Option<i64> = None;

    for lo in ns.children(n, &["w:lvlOverride"]) {
        let mut ilvl: Option<i64> = ns.get(lo, "w:ilvl").and_then(|v| v.trim().parse().ok());

        for so in ns.children(lo, &["w:startOverride"]) {
            if let Some(v) = ns
                .get(so, "w:val")
                .and_then(|v| v.trim().parse::<i64>().ok())
            {
                last_start_override = Some(v);
                start_override_ilvls.push(ilvl);
            }
        }

        if let Some(lvl) = ns.children(lo, &["w:lvl"]).into_iter().next() {
            let nilvl: Option<i64> = ns.get(lvl, "w:ilvl").and_then(|v| v.trim().parse().ok());
            if ilvl.is_none() {
                ilvl = nilvl;
            }
            if let Some(resolved) = ilvl {
                if let Some(existing) = nd.levels.get_mut(&resolved) {
                    existing.read_from_xml(lvl, ns);
                }
                // Python quirk: a brand-new `Level()` built here (when
                // no level exists yet at `resolved`) is read from XML
                // but never stored back into `nd.levels` -- reproduced
                // as-is, see the module docs.
            }
        }
    }

    if let Some(value) = last_start_override {
        for ilvl in start_override_ilvls {
            if let Some(ilvl) = ilvl {
                if let Some(level) = nd.levels.get_mut(&ilvl) {
                    level.start = value;
                }
            }
        }
    }

    nd
}

/// All numbering definitions and instances in a document's
/// `numbering.xml`.
///
/// Port of the Python `Numbering` -- reading (`call`), the two
/// paragraph-style lookups, and `apply_markup` (`to_html.rs`'s
/// `apply_numbering_markup`, since it needs [`crate::dom::Dom`]
/// alongside this type -- see `tables.rs`'s module docs for the same
/// pattern).
#[derive(Debug, Clone, Default)]
pub struct Numbering {
    pub definitions: HashMap<String, NumberingDefinition>,
    pub instances: HashMap<String, NumberingDefinition>,
    pub starts: HashMap<String, HashMap<i64, i64>>,
    pub pic_map: HashMap<String, String>,
    /// Per-abstract-numbering-id running counters, shared across
    /// every `w:num` instance that points at the same
    /// `w:abstractNum` (Python's `self.counters = defaultdict(Counter)`,
    /// keyed by `d.abstract_numbering_definition_id`). Populated and
    /// read by `apply_numbering_markup`'s Phase A.
    pub counters: HashMap<Option<String>, HashMap<i64, i64>>,
}

impl Numbering {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads every numbering definition in `numbering.xml`.
    ///
    /// `numbering_style_links` is `Styles::numbering_style_links` --
    /// see the module docs for why this takes the map directly rather
    /// than the whole (not yet ported) `Styles` collection.
    ///
    /// Port of the Python `Numbering.__call__`.
    pub fn call(
        &mut self,
        root: Node,
        numbering_style_links: &HashMap<String, String>,
        ns: &DocxNamespace,
    ) {
        for npb in ns.children(root, &["w:numPicBullet"]) {
            let Some(npbid) = ns.get(npb, "w:numPicBulletId") else {
                continue;
            };
            for idata in ns.descendants(npb, &["v:imagedata"]) {
                if let Some(rid) = ns.get(idata, "r:id") {
                    self.pic_map.insert(npbid.to_string(), rid.to_string());
                }
            }
        }

        let mut lazy_load: HashMap<String, String> = HashMap::new();
        for an in ns.children(root, &["w:abstractNum"]) {
            let Some(an_id) = ns.get(an, "w:abstractNumId") else {
                continue;
            };
            let nsl = ns.children(an, &["w:numStyleLink"]);
            if let Some(first) = nsl.first() {
                if let Some(val) = ns.get(*first, "w:val") {
                    lazy_load.insert(an_id.to_string(), val.to_string());
                    continue;
                }
            }
            let nd = NumberingDefinition::from_parent(an, ns, Some(an_id.to_string()));
            self.definitions.insert(an_id.to_string(), nd);
        }

        let mut next_pass: HashMap<String, (Option<String>, Node)> = HashMap::new();
        for n in ns.children(root, &["w:num"]) {
            let Some(num_id) = ns.get(n, "w:numId") else {
                continue;
            };
            let mut an_id: Option<String> = None;
            for an in ns.children(n, &["w:abstractNumId"]) {
                if let Some(v) = ns.get(an, "w:val") {
                    an_id = Some(v.to_string());
                }
            }
            let definition = an_id.as_ref().and_then(|id| self.definitions.get(id));
            match definition {
                None => {
                    next_pass.insert(num_id.to_string(), (an_id, n));
                }
                Some(d) => {
                    let instance = create_instance(ns, n, d);
                    self.instances.insert(num_id.to_string(), instance);
                }
            }
        }

        for (an_id, style_link) in &lazy_load {
            let Some(num_id) = numbering_style_links.get(style_link) else {
                // Python indexes `numbering_links[style_link]`
                // directly and would raise `KeyError` on a malformed
                // document; skipping is the safe-Rust equivalent.
                continue;
            };
            if let Some(instance) = self.instances.get(num_id) {
                self.definitions.insert(an_id.clone(), instance.clone());
            }
        }

        for (num_id, (an_id, n)) in &next_pass {
            let Some(d) = an_id.as_ref().and_then(|id| self.definitions.get(id)) else {
                continue;
            };
            let instance = create_instance(ns, *n, d);
            self.instances.insert(num_id.clone(), instance);
        }

        for (num_id, d) in &self.instances {
            let starts: HashMap<i64, i64> =
                d.levels.iter().map(|(lvl, l)| (*lvl, l.start)).collect();
            self.starts.insert(num_id.clone(), starts);
        }
    }

    /// The level a named paragraph style is linked to within a
    /// numbering instance, if any.
    ///
    /// Port of the Python `Numbering.get_pstyle`.
    pub fn get_pstyle(&self, num_id: &str, style_id: &str) -> Option<i64> {
        let d = self.instances.get(num_id)?;
        d.levels
            .iter()
            .find(|(_, lvl)| lvl.para_link.as_deref() == Some(style_id))
            .map(|(ilvl, _)| *ilvl)
    }

    /// The paragraph style attached to one level of a numbering
    /// instance, if any.
    ///
    /// Port of the Python `Numbering.get_para_style`.
    pub fn get_para_style(&self, num_id: &str, lvl: i64) -> Option<&ParagraphStyle> {
        let d = self.instances.get(num_id)?;
        d.levels.get(&lvl)?.paragraph_style.as_ref()
    }

    /// Advances the numbering counter for `levelnum`, resetting any
    /// deeper level that restarts at this point.
    ///
    /// Port of the Python `Numbering.update_counter`.
    pub fn update_counter(
        counter: &mut HashMap<i64, i64>,
        levelnum: i64,
        levels: &HashMap<i64, Level>,
    ) {
        *counter.entry(levelnum).or_insert(0) += 1;
        for (&ilvl, lvl) in levels {
            let restarts_here = match lvl.restart {
                None => ilvl == levelnum + 1,
                Some(r) => r == levelnum + 1,
            };
            if restarts_here {
                counter.insert(ilvl, lvl.start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#;

    fn parse(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("{DOC_OPEN}{body}</w:numbering>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    #[test]
    fn to_roman_matches_python() {
        assert_eq!(to_roman(1), "I");
        assert_eq!(to_roman(4), "IV");
        assert_eq!(to_roman(1994), "MCMXCIV");
        assert_eq!(to_roman(0), "0");
        assert_eq!(to_roman(4000), "4000");
    }

    #[test]
    fn alphabet_wraps_after_z() {
        assert_eq!(alphabet(1, true), "a");
        assert_eq!(alphabet(26, true), "z");
        assert_eq!(alphabet(27, true), "a");
        assert_eq!(alphabet(1, false), "A");
    }

    #[test]
    fn level_treats_a_wingdings_bullet_font_as_a_disc() {
        // A symbol/Wingdings character bullet renders as a plain CSS
        // `disc` rather than keeping the (font-dependent, meaningless
        // outside Word) literal character as a template.
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0">
                <w:numFmt w:val="bullet"/>
                <w:lvlText w:val="x"/>
                <w:rPr><w:rFonts w:ascii="Wingdings"/></w:rPr>
            </w:lvl></w:abstractNum>"#,
        );
        let an = ns.first_child(doc.root_element(), "w:abstractNum").unwrap();
        let lvl = ns.first_child(an, "w:lvl").unwrap();
        let level = Level::from_lvl(lvl, &ns);
        assert!(!level.is_numbered);
        assert_eq!(level.fmt, "disc");
        assert_eq!(level.bullet_template, None);
    }

    #[test]
    fn level_keeps_a_real_bullet_character_as_a_template() {
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0">
                <w:numFmt w:val="bullet"/>
                <w:lvlText w:val="&gt;&gt;"/>
            </w:lvl></w:abstractNum>"#,
        );
        let an = ns.first_child(doc.root_element(), "w:abstractNum").unwrap();
        let lvl = ns.first_child(an, "w:lvl").unwrap();
        let level = Level::from_lvl(lvl, &ns);
        assert!(!level.is_numbered);
        assert_eq!(level.bullet_template.as_deref(), Some(">>"));
    }

    #[test]
    fn level_reads_decimal_numbering_with_template() {
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0">
                <w:start w:val="3"/>
                <w:numFmt w:val="decimal"/>
                <w:lvlText w:val="%1."/>
            </w:lvl></w:abstractNum>"#,
        );
        let an = ns.first_child(doc.root_element(), "w:abstractNum").unwrap();
        let lvl = ns.first_child(an, "w:lvl").unwrap();
        let level = Level::from_lvl(lvl, &ns);
        assert!(level.is_numbered);
        assert_eq!(level.start, 3);
        assert_eq!(level.fmt, "decimal");
        // "%1." matches the bare-counter pattern, so no template is stored.
        assert_eq!(level.num_template, None);
    }

    #[test]
    fn level_keeps_a_real_multi_level_template() {
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="1">
                <w:numFmt w:val="lowerLetter"/>
                <w:lvlText w:val="%1.%2)"/>
            </w:lvl></w:abstractNum>"#,
        );
        let an = ns.first_child(doc.root_element(), "w:abstractNum").unwrap();
        let lvl = ns.first_child(an, "w:lvl").unwrap();
        let level = Level::from_lvl(lvl, &ns);
        assert_eq!(level.fmt, "lower-alpha");
        assert_eq!(level.num_template.as_deref(), Some("%1.%2)"));
    }

    #[test]
    fn format_template_renders_nested_counters() {
        let level = Level {
            fmt: "decimal".to_string(),
            ..Level::new()
        };
        let mut counter = HashMap::new();
        counter.insert(0, 2);
        counter.insert(1, 3);
        assert_eq!(level.format_template(&counter, 1, "%1.%2"), "1.3\u{a0}");
    }

    #[test]
    fn update_counter_resets_deeper_levels_on_restart() {
        let mut levels = HashMap::new();
        levels.insert(1, Level::new());
        let mut counter = HashMap::new();
        counter.insert(1, 5);
        Numbering::update_counter(&mut counter, 0, &levels);
        assert_eq!(counter.get(&0), Some(&1));
        assert_eq!(
            counter.get(&1),
            Some(&0),
            "level 1 restarts after level 0 advances"
        );
    }

    #[test]
    fn call_reads_definitions_and_instances_with_overrides() {
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="1">
                <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl>
            </w:abstractNum>
            <w:num w:numId="9">
                <w:abstractNumId w:val="1"/>
                <w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride>
            </w:num>"#,
        );
        let mut numbering = Numbering::new();
        numbering.call(doc.root_element(), &HashMap::new(), &ns);
        let instance = numbering.instances.get("9").expect("instance created");
        assert_eq!(instance.levels[&0].start, 5, "startOverride applied");
        assert_eq!(numbering.starts["9"][&0], 5);
    }

    #[test]
    fn call_resolves_style_linked_abstract_nums_via_the_supplied_map() {
        let (doc, ns) = parse(
            r#"<w:abstractNum w:abstractNumId="1">
                <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl>
            </w:abstractNum>
            <w:abstractNum w:abstractNumId="2">
                <w:numStyleLink w:val="LinkedStyle"/>
            </w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="1"/></w:num>"#,
        );
        let mut links = HashMap::new();
        links.insert("LinkedStyle".to_string(), "1".to_string());
        let mut numbering = Numbering::new();
        numbering.call(doc.root_element(), &links, &ns);
        assert!(numbering.definitions.contains_key("2"));
    }

    #[test]
    fn pic_map_reads_vml_image_data_relationship_ids() {
        let (doc, ns) = parse(
            r#"<w:numPicBullet w:numPicBulletId="0">
                <w:pict><v:shape><v:imagedata r:id="rId7"/></v:shape></w:pict>
            </w:numPicBullet>"#,
        );
        let mut numbering = Numbering::new();
        numbering.call(doc.root_element(), &HashMap::new(), &ns);
        assert_eq!(numbering.pic_map.get("0").map(String::as_str), Some("rId7"));
    }
}
