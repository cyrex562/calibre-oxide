//! Port of `old_src/src/calibre/ebooks/pdf/render/fonts.py` (251 lines).
//!
//! Embeds subsetted, CID-keyed (`/Identity-H` encoded) fonts into the
//! PDF: `FontStream`/`CMap` (the two `Stream` subclasses), `Font`
//! (descriptor construction, width table, `ToUnicode` CMap), and
//! `FontManager` (dedup/caching + the standard 14 fonts).
//!
//! # Scope: what's real vs. blocked
//!
//! `Font.__init__` and `Font.embed` read from a `metrics` object
//! (`calibre.utils.fonts.sfnt.metrics.FontMetrics`) - a whole separate,
//! never-ported module tree (`calibre/utils/fonts/sfnt/*`; not to be
//! confused with `crate::oeb::polish::check::fonts`'s `sfnt` submodule,
//! which is a narrow *read-only* name/embeddability-flag parser for a
//! different purpose and doesn't provide this metrics surface - see that
//! file's own doc comment). [`FontMetrics`] here is a plain trait mirror
//! of that Python class's surface (`is_otf`, `postscript_name`,
//! `pdf_bbox`, `post.italic_angle`, `pdf_ascent`/`pdf_descent`/
//! `pdf_capheight`/`pdf_avg_width`/`pdf_stemv`, `glyph_map`,
//! `pdf_scale`/`glyph_widths`, `os2.zero_fstype`, and the two hooks used
//! only inside the blocked step below), documented as the stand-in for
//! that unported class - not itself a gap, since [`Font::new`],
//! [`Font::write_widths`], and [`Font::write_to_unicode`] are real,
//! tested logic that only *reads* from it.
//!
//! The one genuine gap: `Font.embed`'s call to
//! `pdf_subset(self.metrics.sfnt, self.used_glyphs)` (from
//! `calibre.utils.fonts.sfnt.subset`, also unported) actually subsets
//! the font's glyph table - real glyph-table surgery, well beyond a
//! narrow read. [`Font::embed`] is restructured (vs. Python's linear
//! method) so this is the *only* `todo!()`'d step:
//! [`Font::subset_and_write_font_data`] bundles the subset call together
//! with registering/writing the `FontFile`/`FontFile3` stream (which
//! also needs the metrics object's live sfnt-table writer, itself only
//! reachable after subsetting) - while [`Font::write_widths`] and
//! [`Font::write_to_unicode`] (the width table and `ToUnicode` CMap) run
//! as real, independently callable and tested steps, both before and
//! regardless of that gap. `FontManager::embed_fonts`/`Font::embed`
//! still call all three in Python's original order, so calling either of
//! those *drivers* end-to-end still hits the `todo!()` - only the two
//! leaf methods are safe to call standalone.

use std::collections::BTreeSet;

use indexmap::IndexMap;
use lazy_static::lazy_static;

use super::common::{Array, Dictionary, Name, PdfString, Reference, Stream, StreamLike};
use super::serialize::IndirectObjects;

lazy_static! {
    /// Port of `STANDARD_FONTS` (fonts.py lines 18-22): the PDF standard
    /// 14 font names, embeddable without a font program.
    pub static ref STANDARD_FONTS: std::collections::HashSet<&'static str> = [
        "Times-Roman", "Helvetica", "Courier", "Symbol", "Times-Bold",
        "Helvetica-Bold", "Courier-Bold", "ZapfDingbats", "Times-Italic",
        "Helvetica-Oblique", "Courier-Oblique", "Times-BoldItalic",
        "Helvetica-BoldOblique", "Courier-BoldOblique",
    ].into_iter().collect();
}

// ==========================================================================
// FontMetrics - stand-in for calibre.utils.fonts.sfnt.metrics.FontMetrics
// ==========================================================================

/// Plain trait mirror of `calibre.utils.fonts.sfnt.metrics.FontMetrics`
/// (an entire unported module, `calibre/utils/fonts/sfnt/metrics.py`).
/// See the module doc comment.
pub trait FontMetrics {
    fn is_otf(&self) -> bool;
    /// `metrics.postscript_name`; `None` mirrors the Python
    /// `try/except` fallback to a random UUID at both of its two call
    /// sites (`Font.__init__`, `Font.write_to_unicode`).
    fn postscript_name(&self) -> Option<String>;
    /// `metrics.names.get('full_name', 'Unknown')`, used only in debug
    /// messages inside the blocked subsetting step.
    fn full_name(&self) -> String {
        "Unknown".to_string()
    }
    fn pdf_bbox(&self) -> [f64; 4];
    /// `metrics.post.italic_angle`.
    fn italic_angle(&self) -> f64;
    fn pdf_ascent(&self) -> f64;
    fn pdf_descent(&self) -> f64;
    fn pdf_capheight(&self) -> f64;
    fn pdf_avg_width(&self) -> f64;
    fn pdf_stemv(&self) -> f64;
    /// `metrics.glyph_map`: glyph id -> the Unicode string it represents
    /// (usually one character), used to build the `ToUnicode` CMap.
    fn glyph_map(&self) -> std::collections::BTreeMap<u32, String>;
    fn pdf_scale(&self, w: f64) -> f64;
    /// `metrics.glyph_widths(glyphs)`: raw (unscaled) advance widths for
    /// the given glyph ids, same order/length as `glyphs`.
    fn glyph_widths(&self, glyphs: &[u32]) -> Vec<f64>;
}

// ==========================================================================
// FontStream / to_hex_string / CMap (fonts.py lines 48-115)
// ==========================================================================

/// Port of `FontStream` (fonts.py lines 48-57): the embedded font
/// program stream, `Length1` + (for OTF) `/Subtype /CIDFontType0C`.
#[derive(Debug, Clone)]
pub struct FontStream {
    pub inner: Stream,
    pub is_otf: bool,
}

impl FontStream {
    pub fn new(is_otf: bool, compress: bool) -> Self {
        FontStream {
            inner: Stream::new(compress),
            is_otf,
        }
    }
}

impl StreamLike for FontStream {
    fn stream(&self) -> &Stream {
        &self.inner
    }
    fn extra_keys(&self) -> Vec<(String, super::common::PdfObj)> {
        let mut v = vec![(
            "Length1".to_string(),
            (self.inner.getvalue().len() as i64).into(),
        )];
        if self.is_otf {
            v.push(("Subtype".to_string(), Name::new("CIDFontType0C").into()));
        }
        v
    }
}

/// Port of `to_hex_string` (fonts.py lines 60-64): a 4-digit lowercase
/// hex code point, as used inside the CMap's `<hex>` tokens.
pub fn to_hex_string(c: u32) -> String {
    format!("{c:04x}")
}

/// Port of `CMap` (fonts.py lines 67-114): a `ToUnicode` CMap stream
/// mapping glyph ids back to the Unicode text they represent, in
/// `beginbfchar`/`endbfchar` blocks of up to 100 entries.
#[derive(Debug, Clone)]
pub struct CMap {
    pub inner: Stream,
}

const CMAP_SKELETON_HEAD: &str = "/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CMapName ";
const CMAP_SKELETON_MID: &str = "-cmap def\n\
/CMapType 2 def\n\
/CIDSystemInfo <<\n\
/Registry (Adobe)\n\
/Ordering (UCS)\n\
/Supplement 0\n\
>> def\n\
1 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n";
const CMAP_SKELETON_TAIL: &str = "\nendcmap\n\
CMapName currentdict /CMap defineresource pop\n\
end\n\
end\n";

impl CMap {
    /// Port of `CMap.__init__` (fonts.py lines 90-114).
    pub fn new(
        name: &str,
        glyph_map: &std::collections::BTreeMap<u32, String>,
        compress: bool,
    ) -> Self {
        let mut maps: Vec<IndexMap<String, String>> = Vec::new();
        let mut current: IndexMap<String, String> = IndexMap::new();
        for (&glyph_id, chars) in glyph_map {
            if current.len() > 99 {
                maps.push(std::mem::take(&mut current));
            }
            let val: String = chars.chars().map(|c| to_hex_string(c as u32)).collect();
            current.insert(format!("<{}>", to_hex_string(glyph_id)), format!("<{val}>"));
        }
        if !current.is_empty() {
            maps.push(current);
        }
        let mut mapping = String::new();
        for m in &maps {
            if !mapping.is_empty() {
                mapping.push('\n');
            }
            let meat: Vec<String> = m.iter().map(|(k, v)| format!("{k} {v}")).collect();
            mapping.push_str(&format!(
                "{} beginbfchar\n{}\nendbfchar",
                m.len(),
                meat.join("\n")
            ));
        }
        let name = if name.is_ascii() && !name.is_empty() {
            name.to_string()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let mut inner = Stream::new(compress);
        inner.write(format!(
            "{CMAP_SKELETON_HEAD}{name}{CMAP_SKELETON_MID}{mapping}{CMAP_SKELETON_TAIL}"
        ));
        CMap { inner }
    }
}

impl StreamLike for CMap {
    fn stream(&self) -> &Stream {
        &self.inner
    }
}

// ==========================================================================
// Font (fonts.py lines 117-215)
// ==========================================================================

/// Port of `Font.subset_tag` construction (fonts.py lines 122-124): a
/// deterministic 6-letter subset prefix (`AAAAAA`, `AAAABH`, ...) derived
/// from the font's index, matching the Python source's octal-digit ->
/// letter mapping exactly (including its always-present leading `"0"`
/// digit, an artifact of `oct(num)`'s `"0o"` prefix).
fn subset_tag(num: u32) -> String {
    let digits = format!("0{num:o}");
    let mut letters: String = digits
        .chars()
        .map(|c| {
            let d = c.to_digit(8).expect("octal digit");
            (b'A' + d as u8) as char
        })
        .collect();
    while letters.len() < 6 {
        letters.insert(0, 'A');
    }
    letters
}

/// Port of `Font` (fonts.py lines 117-215). See the module doc comment
/// for the `embed`/subsetting restructuring.
pub struct Font {
    pub metrics: Box<dyn FontMetrics>,
    pub compress: bool,
    pub is_otf: bool,
    pub subset_tag: String,
    pub base_font_name: String,
    pub font_descriptor_ref: Reference,
    pub descendant_font_ref: Reference,
    /// Not yet added to the arena - [`super::FontManager::add_font`]
    /// does that immediately after construction (mirrors Python's
    /// `FontManager.add_font` calling `self.objects.add(font.font_dict)`
    /// right after `Font(...)` returns) and fills in
    /// [`Font::font_dict_ref`].
    pub font_dict: Dictionary,
    pub font_dict_ref: Option<Reference>,
    pub used_glyphs: BTreeSet<u32>,
}

impl Font {
    /// Port of `Font.__init__` (fonts.py lines 119-164).
    pub fn new(
        metrics: Box<dyn FontMetrics>,
        num: u32,
        objects: &mut IndirectObjects,
        compress: bool,
    ) -> Font {
        let is_otf = metrics.is_otf();
        let tag = subset_tag(num);
        let psname = metrics
            .postscript_name()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let base_font_name = format!("{tag}+{psname}");

        let mut font_descriptor = Dictionary::new();
        font_descriptor.insert("Type", Name::new("FontDescriptor"));
        font_descriptor.insert("FontName", Name::new(base_font_name.clone()));
        font_descriptor.insert("Flags", 0b100i64); // Symbolic font
        font_descriptor.insert("FontBBox", {
            let mut a = Array::new();
            a.extend(metrics.pdf_bbox());
            a
        });
        font_descriptor.insert("ItalicAngle", metrics.italic_angle());
        font_descriptor.insert("Ascent", metrics.pdf_ascent());
        font_descriptor.insert("Descent", metrics.pdf_descent());
        font_descriptor.insert("CapHeight", metrics.pdf_capheight());
        font_descriptor.insert("AvgWidth", metrics.pdf_avg_width());
        font_descriptor.insert("StemV", metrics.pdf_stemv());
        let font_descriptor_ref = objects.add_dict(font_descriptor);

        let mut descendant_font = Dictionary::new();
        descendant_font.insert("Type", Name::new("Font"));
        descendant_font.insert(
            "Subtype",
            Name::new(format!("CIDFontType{}", if is_otf { "0" } else { "2" })),
        );
        descendant_font.insert("BaseFont", Name::new(base_font_name.clone()));
        descendant_font.insert("FontDescriptor", font_descriptor_ref);
        let mut cid_sysinfo = Dictionary::new();
        cid_sysinfo.insert("Registry", PdfString::new("Adobe"));
        cid_sysinfo.insert("Ordering", PdfString::new("Identity"));
        cid_sysinfo.insert("Supplement", 0i64);
        descendant_font.insert("CIDSystemInfo", cid_sysinfo);
        if !is_otf {
            descendant_font.insert("CIDToGIDMap", Name::new("Identity"));
        }
        let descendant_font_ref = objects.add_dict(descendant_font);

        let mut font_dict = Dictionary::new();
        font_dict.insert("Type", Name::new("Font"));
        font_dict.insert("Subtype", Name::new("Type0"));
        font_dict.insert("Encoding", Name::new("Identity-H"));
        font_dict.insert("BaseFont", Name::new(base_font_name.clone()));
        font_dict.insert("DescendantFonts", {
            let mut a = Array::new();
            a.push(descendant_font_ref);
            a
        });

        Font {
            metrics,
            compress,
            is_otf,
            subset_tag: tag,
            base_font_name,
            font_descriptor_ref,
            descendant_font_ref,
            font_dict,
            font_dict_ref: None,
            used_glyphs: BTreeSet::new(),
        }
    }

    /// Port of `Font.write_widths` (fonts.py lines 195-215). Real,
    /// independently testable.
    pub fn write_widths(&self, objects: &mut IndirectObjects) {
        let mut glyph_ids = self.used_glyphs.clone();
        glyph_ids.insert(0);
        let glyphs: Vec<u32> = glyph_ids.into_iter().collect(); // BTreeSet -> sorted ascending
        let raw_widths = self.metrics.glyph_widths(&glyphs);
        let widths: Vec<(u32, f64)> = glyphs
            .iter()
            .copied()
            .zip(raw_widths.iter().map(|&w| self.metrics.pdf_scale(w)))
            .collect();

        // Counter.most_common(1): tie-broken by first-seen order here,
        // matching CPython's stable sort for small inputs.
        let mut counts: IndexMap<u64, usize> = IndexMap::new();
        for &(_, w) in &widths {
            *counts.entry(w.to_bits()).or_insert(0) += 1;
        }
        let mut most_common_bits = 0u64;
        let mut best = 0usize;
        for (&bits, &count) in &counts {
            if count > best {
                best = count;
                most_common_bits = bits;
            }
        }
        let most_common = f64::from_bits(most_common_bits);

        let filtered: Vec<(u32, f64)> = widths
            .into_iter()
            .filter(|&(_, w)| w.to_bits() != most_common_bits)
            .collect();

        let mut groups = Array::new();
        let mut i = 0usize;
        while i < filtered.len() {
            let key0 = i as i64 - filtered[i].0 as i64;
            let mut j = i;
            while j + 1 < filtered.len() && (j + 1) as i64 - filtered[j + 1].0 as i64 == key0 {
                j += 1;
            }
            let run = &filtered[i..=j];
            let min_g = run.iter().map(|&(g, _)| g).min().unwrap();
            let max_g = run.iter().map(|&(g, _)| g).max().unwrap();
            let first_bits = run[0].1.to_bits();
            let all_same = run.iter().all(|&(_, w)| w.to_bits() == first_bits);
            if all_same && run.len() > 1 {
                groups.push(min_g as i64);
                groups.push(max_g as i64);
                groups.push(run[0].1);
            } else {
                groups.push(min_g as i64);
                let mut arr = Array::new();
                arr.extend(run.iter().map(|&(_, w)| w));
                groups.push(arr);
            }
            i = j + 1;
        }
        let groups_ref = objects.add_array(groups);

        let d = objects.get_dict_mut(&self.descendant_font_ref);
        d.insert("DW", most_common);
        d.insert("W", groups_ref);
    }

    /// Port of `Font.write_to_unicode` (fonts.py lines 187-193). Real,
    /// independently testable.
    pub fn write_to_unicode(&self, objects: &mut IndirectObjects) {
        let name = self
            .metrics
            .postscript_name()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let cmap = CMap::new(&name, &self.metrics.glyph_map(), self.compress);
        let cmap_ref = objects.add_stream(cmap);
        let font_dict_ref = self
            .font_dict_ref
            .expect("font_dict must be added by FontManager::add_font first");
        objects
            .get_dict_mut(&font_dict_ref)
            .insert("ToUnicode", cmap_ref);
    }

    /// Port of the tail of `Font.embed` (fonts.py lines 166-185): register
    /// and write the (subsetted) font program. This is the one genuinely
    /// blocked step - see the module doc comment.
    pub fn subset_and_write_font_data(
        &mut self,
        _objects: &mut IndirectObjects,
        _debug: &mut dyn FnMut(&str),
    ) {
        todo!(
            "placeholder: needs calibre.utils.fonts.sfnt.subset.pdf_subset (glyph-table subsetting) \
             and the live sfnt-table writer on calibre.utils.fonts.sfnt.metrics.FontMetrics - both \
             from the unported calibre.utils.fonts.sfnt.* module tree, see docs/modules_to_port.md"
        )
    }

    /// Port of `Font.embed` (fonts.py lines 166-185): writes widths and
    /// the `ToUnicode` CMap for real, then hits the blocked subsetting
    /// step. Reordered relative to Python (which registers the
    /// `FontFile` stream first) since nothing here observably depends on
    /// indirect-object insertion order - see the module doc comment.
    pub fn embed(&mut self, objects: &mut IndirectObjects, debug: &mut dyn FnMut(&str)) {
        self.write_widths(objects);
        self.write_to_unicode(objects);
        self.subset_and_write_font_data(objects, debug);
    }
}

// ==========================================================================
// FontManager (fonts.py lines 218-252)
// ==========================================================================

/// Port of `FontManager` (fonts.py lines 218-252).
///
/// Python's `add_font(font_metrics, glyph_ids)` dedups on the
/// `font_metrics` object's own hash/equality. Since [`FontMetrics`] is a
/// trait object here (no natural `Eq`/`Hash`), callers supply an
/// explicit `key` identifying the font (e.g. its embed-file path or
/// PostScript name) - a direct, faithful translation of "dedup by
/// object identity/value" into an explicit Rust key.
pub struct FontManager {
    pub compress: bool,
    std_map: IndexMap<String, Reference>,
    font_map: IndexMap<String, usize>,
    pub fonts: Vec<Font>,
}

impl FontManager {
    /// Port of `FontManager.__init__` (fonts.py lines 220-225).
    pub fn new(compress: bool) -> Self {
        FontManager {
            compress,
            std_map: IndexMap::new(),
            font_map: IndexMap::new(),
            fonts: Vec::new(),
        }
    }

    /// Port of `FontManager.add_font` (fonts.py lines 227-236).
    pub fn add_font(
        &mut self,
        key: impl Into<String>,
        metrics: Box<dyn FontMetrics>,
        glyph_ids: &BTreeSet<u32>,
        objects: &mut IndirectObjects,
    ) -> Reference {
        let key = key.into();
        if !self.font_map.contains_key(&key) {
            let num = self.fonts.len() as u32;
            let mut font = Font::new(metrics, num, objects, self.compress);
            let font_dict = std::mem::take(&mut font.font_dict);
            let fontref = objects.add_dict(font_dict);
            font.font_dict_ref = Some(fontref);
            self.fonts.push(font);
            self.font_map.insert(key.clone(), self.fonts.len() - 1);
        }
        let idx = self.font_map[&key];
        self.fonts[idx]
            .used_glyphs
            .extend(glyph_ids.iter().copied());
        self.fonts[idx].font_dict_ref.expect("just inserted above")
    }

    /// Port of `FontManager.add_standard_font` (fonts.py lines 238-247).
    pub fn add_standard_font(
        &mut self,
        name: &str,
        objects: &mut IndirectObjects,
    ) -> anyhow::Result<Reference> {
        if !STANDARD_FONTS.contains(name) {
            anyhow::bail!("{name} is not a standard font");
        }
        if !self.std_map.contains_key(name) {
            let mut d = Dictionary::new();
            d.insert("Type", Name::new("Font"));
            d.insert("Subtype", Name::new("Type1"));
            d.insert("BaseFont", Name::new(name));
            let r = objects.add_dict(d);
            self.std_map.insert(name.to_string(), r);
        }
        Ok(self.std_map[name])
    }

    /// Port of `FontManager.embed_fonts` (fonts.py lines 249-252). Calls
    /// [`Font::embed`] per font, so - like the Python original - it hits
    /// the blocked subsetting step if any font has been registered; the
    /// point of the restructuring above is that [`Font::write_widths`]/
    /// [`Font::write_to_unicode`] remain independently real and testable
    /// even though this driver method is not.
    pub fn embed_fonts(&mut self, objects: &mut IndirectObjects, mut debug: impl FnMut(&str)) {
        for font in &mut self.fonts {
            font.embed(objects, &mut debug);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::render::common::PdfObj;

    struct MockMetrics {
        otf: bool,
        ps_name: Option<String>,
        widths: std::collections::BTreeMap<u32, f64>,
    }

    impl FontMetrics for MockMetrics {
        fn is_otf(&self) -> bool {
            self.otf
        }
        fn postscript_name(&self) -> Option<String> {
            self.ps_name.clone()
        }
        fn pdf_bbox(&self) -> [f64; 4] {
            [-100.0, -200.0, 1000.0, 900.0]
        }
        fn italic_angle(&self) -> f64 {
            0.0
        }
        fn pdf_ascent(&self) -> f64 {
            800.0
        }
        fn pdf_descent(&self) -> f64 {
            -200.0
        }
        fn pdf_capheight(&self) -> f64 {
            700.0
        }
        fn pdf_avg_width(&self) -> f64 {
            500.0
        }
        fn pdf_stemv(&self) -> f64 {
            80.0
        }
        fn glyph_map(&self) -> std::collections::BTreeMap<u32, String> {
            let mut m = std::collections::BTreeMap::new();
            m.insert(3, "A".to_string());
            m.insert(4, "B".to_string());
            m
        }
        fn pdf_scale(&self, w: f64) -> f64 {
            w / 2.0
        }
        fn glyph_widths(&self, glyphs: &[u32]) -> Vec<f64> {
            glyphs
                .iter()
                .map(|g| *self.widths.get(g).unwrap_or(&600.0))
                .collect()
        }
    }

    fn mock_metrics() -> Box<dyn FontMetrics> {
        let mut widths = std::collections::BTreeMap::new();
        widths.insert(0, 600.0);
        widths.insert(3, 600.0);
        widths.insert(4, 800.0);
        widths.insert(5, 600.0);
        Box::new(MockMetrics {
            otf: false,
            ps_name: Some("MockFont".to_string()),
            widths,
        })
    }

    #[test]
    fn subset_tag_zero() {
        assert_eq!(subset_tag(0), "AAAAAA");
    }

    #[test]
    fn subset_tag_fifteen() {
        assert_eq!(subset_tag(15), "AAAABH");
    }

    #[test]
    fn subset_tag_one() {
        assert_eq!(subset_tag(1), "AAAAAB");
    }

    #[test]
    fn to_hex_string_pads_to_four() {
        assert_eq!(to_hex_string(0xAB), "00ab");
        assert_eq!(to_hex_string(0xFFFF), "ffff");
    }

    #[test]
    fn font_new_builds_descriptor_and_font_dict() {
        let mut objects = IndirectObjects::new();
        let font = Font::new(mock_metrics(), 0, &mut objects, false);
        assert_eq!(font.subset_tag, "AAAAAA");
        assert_eq!(font.base_font_name, "AAAAAA+MockFont");
        let descriptor = objects.get_dict(&font.font_descriptor_ref);
        assert_eq!(
            descriptor.get("Type"),
            Some(&PdfObj::Name(Name::new("FontDescriptor")))
        );
        assert_eq!(descriptor.get("StemV"), Some(&PdfObj::Real(80.0)));
        let descendant = objects.get_dict(&font.descendant_font_ref);
        assert_eq!(
            descendant.get("Subtype"),
            Some(&PdfObj::Name(Name::new("CIDFontType2")))
        );
        assert!(descendant.contains_key("CIDToGIDMap"));
        assert!(font.font_dict.contains_key("DescendantFonts"));
    }

    #[test]
    fn font_new_otf_has_no_cid_to_gid_map_and_type0c_subtype() {
        let mut objects = IndirectObjects::new();
        let widths = std::collections::BTreeMap::new();
        let metrics: Box<dyn FontMetrics> = Box::new(MockMetrics {
            otf: true,
            ps_name: Some("OtfFont".to_string()),
            widths,
        });
        let font = Font::new(metrics, 0, &mut objects, false);
        let descendant = objects.get_dict(&font.descendant_font_ref);
        assert_eq!(
            descendant.get("Subtype"),
            Some(&PdfObj::Name(Name::new("CIDFontType0")))
        );
        assert!(!descendant.contains_key("CIDToGIDMap"));
    }

    #[test]
    fn write_widths_sets_dw_and_w_groups() {
        let mut objects = IndirectObjects::new();
        let mut font = Font::new(mock_metrics(), 0, &mut objects, false);
        font.used_glyphs.insert(3);
        font.used_glyphs.insert(4);
        font.used_glyphs.insert(5);
        font.write_widths(&mut objects);
        let descendant = objects.get_dict(&font.descendant_font_ref);
        // glyph 0,3,5 scale to 300 (most common, 3 occurrences); glyph 4 scales to 400 (odd one out).
        assert_eq!(descendant.get("DW"), Some(&PdfObj::Real(300.0)));
        assert!(descendant.contains_key("W"));
    }

    #[test]
    fn write_to_unicode_sets_cmap_reference() {
        let mut objects = IndirectObjects::new();
        let mut font = Font::new(mock_metrics(), 0, &mut objects, false);
        let font_dict = std::mem::take(&mut font.font_dict);
        font.font_dict_ref = Some(objects.add_dict(font_dict));
        font.write_to_unicode(&mut objects);
        let d = objects.get_dict(&font.font_dict_ref.unwrap());
        assert!(d.contains_key("ToUnicode"));
    }

    #[test]
    #[should_panic(expected = "placeholder")]
    fn subset_and_write_font_data_is_a_documented_gap() {
        let mut objects = IndirectObjects::new();
        let mut font = Font::new(mock_metrics(), 0, &mut objects, false);
        font.subset_and_write_font_data(&mut objects, &mut |_| {});
    }

    #[test]
    fn cmap_new_builds_bfchar_blocks() {
        let mut glyph_map = std::collections::BTreeMap::new();
        glyph_map.insert(3u32, "A".to_string());
        glyph_map.insert(4u32, "B".to_string());
        let cmap = CMap::new("TestFont", &glyph_map, false);
        let text = String::from_utf8(cmap.inner.getvalue().to_vec()).unwrap();
        assert!(text.contains("/CMapName TestFont-cmap def"));
        assert!(text.contains("beginbfchar"));
        assert!(text.contains("<0003> <0041>"));
        assert!(text.contains("<0004> <0042>"));
    }

    #[test]
    fn font_manager_add_font_dedupes_by_key() {
        let mut objects = IndirectObjects::new();
        let mut fm = FontManager::new(false);
        let mut glyphs1 = BTreeSet::new();
        glyphs1.insert(3);
        let mut glyphs2 = BTreeSet::new();
        glyphs2.insert(4);
        let ref1 = fm.add_font("font-a", mock_metrics(), &glyphs1, &mut objects);
        let ref2 = fm.add_font("font-a", mock_metrics(), &glyphs2, &mut objects);
        assert_eq!(ref1, ref2);
        assert_eq!(fm.fonts.len(), 1);
        assert_eq!(fm.fonts[0].used_glyphs.len(), 2);
    }

    #[test]
    fn font_manager_add_standard_font_rejects_unknown_names() {
        let mut objects = IndirectObjects::new();
        let mut fm = FontManager::new(false);
        assert!(fm.add_standard_font("NotAFont", &mut objects).is_err());
        assert!(fm.add_standard_font("Helvetica", &mut objects).is_ok());
    }

    #[test]
    fn font_manager_add_standard_font_dedupes() {
        let mut objects = IndirectObjects::new();
        let mut fm = FontManager::new(false);
        let r1 = fm.add_standard_font("Courier", &mut objects).unwrap();
        let r2 = fm.add_standard_font("Courier", &mut objects).unwrap();
        assert_eq!(r1, r2);
    }
}
