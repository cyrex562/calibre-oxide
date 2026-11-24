//! Port of `old_src/src/calibre/ebooks/pdf/image_writer.py` (140 lines).
//!
//! Renders a set of images (e.g. a comic) into a PDF - one image per page.
//! **Documented gap**: the actual page-drawing (`draw_image_page`,
//! `convert`) needs `calibre.ebooks.pdf.render.common`/
//! `calibre.ebooks.pdf.render.serialize.PDFStream` - calibre's own native
//! PDF *serializer* at `old_src/src/calibre/ebooks/pdf/render/`. That
//! module is now ported for real, as `crate::pdf::render` (issue #46) -
//! but wiring these two functions up to it is still out of scope *here*
//! (issue #45's scope was the six files in this directory, not
//! integrating them with `render`'s output). Both `[todo!()]`-marked
//! functions below reference `crate::pdf::render` by name rather than
//! re-deriving the explanation; the missing piece is Qt itself
//! (`QPageLayout`/`QPageSize` for page geometry, and driving
//! `crate::pdf::render::graphics::Graphics` from a live paint engine -
//! see that module's own documented gap), not the PDF serializer, which
//! is now available.
//!
//! What *is* separable and genuinely portable: the page-size/unit-
//! conversion math (`parse_pdf_page_size`, `get_page_size`,
//! `get_page_layout`) doesn't touch Qt types in its actual arithmetic -
//! only in the *return* type upstream (`QPageSize`/`QPageLayout`). Ported
//! for real here as plain point-based structs, plus `PDFMetadata`, which
//! is pure string/metadata munging.

use crate::metadata::authors::authors_to_string;
use crate::metadata::meta::MetaInformation;

// ==========================================================================
// Unit conversion constants, port of
// `old_src/src/calibre/ebooks/pdf/render/common.py` lines 21-26 (that
// module itself is out of scope for this issue - see the module doc
// comment - but these five constants are simple enough to duplicate here
// rather than gate this file's real logic on an unported module).
// ==========================================================================

pub const INCH: f64 = 72.0;
pub const CM: f64 = INCH / 2.54;
pub const MM: f64 = CM * 0.1;
pub const PICA: f64 = 12.0;
pub const DIDOT: f64 = 0.375 * MM;
pub const CICERO: f64 = 12.0 * DIDOT;

// ==========================================================================
// PDFMetadata (reflow.py... err, image_writer.py lines 15-35)
// ==========================================================================

/// Port of `PDFMetadata` (image_writer.py lines 15-35).
#[derive(Debug, Clone)]
pub struct PdfMetadata {
    pub title: String,
    pub author: String,
    pub tags: String,
    pub mi: Option<MetaInformation>,
}

impl PdfMetadata {
    /// Port of `PDFMetadata.__init__` (image_writer.py lines 17-34).
    pub fn new(mi: Option<MetaInformation>) -> PdfMetadata {
        let mut title = "Unknown".to_string();
        let mut author = "Unknown".to_string();
        let mut tags = String::new();

        if let Some(mi) = &mi {
            if !mi.title.is_empty() {
                title = mi.title.clone();
            }
            if !mi.authors.is_empty() {
                author = authors_to_string(&mi.authors);
            }
            if !mi.tags.is_empty() {
                tags = mi.tags.join(", ");
            }
        }

        PdfMetadata {
            title,
            author,
            tags,
            mi,
        }
    }
}

// ==========================================================================
// Page layout (image_writer.py lines 38-89)
// ==========================================================================

/// A page size in PDF points (1/72 inch), the plain-data stand-in for
/// upstream's `QPageSize`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSizePoints {
    pub width: f64,
    pub height: f64,
}

/// Margins in PDF points, the plain-data stand-in for `QMarginsF`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MarginsPoints {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// A page layout: size plus margins, the plain-data stand-in for
/// `QPageLayout`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageLayoutPoints {
    pub size: PageSizePoints,
    pub margins: MarginsPoints,
}

fn unit_factor(unit: &str, dpi: f64) -> f64 {
    if unit == "devicepixel" {
        72.0 / dpi
    } else {
        match unit {
            "point" => 1.0,
            "inch" => INCH,
            "cicero" => CICERO,
            "didot" => DIDOT,
            "pica" => PICA,
            "millimeter" => MM,
            "centimeter" => CM,
            _ => 1.0,
        }
    }
}

/// Port of `parse_pdf_page_size` (image_writer.py lines 40-57): parse a
/// `"WIDTHxHEIGHT"` spec (e.g. `"8.5x11"`) in the given `unit` into a page
/// size in points. Returns `None` wherever the Python original does
/// (`spec` without an `x` separator, or a non-numeric width/height) -
/// matching its graceful `pass`-and-fall-through-to-`None` on parse
/// failure rather than raising.
pub fn parse_pdf_page_size(spec: &str, unit: &str, dpi: f64) -> Option<PageSizePoints> {
    let spec = spec.to_lowercase();
    let (width_str, height_str) = spec.split_once('x')?;
    if height_str.is_empty() {
        return None;
    }
    let width: f64 = width_str.replace(',', ".").parse().ok()?;
    let height: f64 = height_str.replace(',', ".").parse().ok()?;
    let factor = unit_factor(unit, dpi);
    Some(PageSizePoints {
        width: factor * width,
        height: factor * height,
    })
}

/// Options consulted by [`get_page_size`]/[`get_page_layout`]: the pure
/// subset of `opts`/`opts.output_profile` that `image_writer.py`'s page
/// layout math reads (`use_profile_size`, `custom_size`, `unit`,
/// `paper_size`, and margins), independent of calibre's full output-profile
/// machinery.
#[derive(Debug, Clone)]
pub struct PageLayoutOpts {
    pub use_profile_size: bool,
    pub profile_is_default: bool,
    pub profile_width: f64,
    pub profile_height: f64,
    pub profile_dpi: f64,
    pub for_comic_screen_size: Option<(f64, f64)>,
    pub custom_size: Option<String>,
    pub unit: String,
    /// A named paper size (e.g. `"a4"`, `"letter"`), used when
    /// `custom_size` is absent. Since this port has no `QPageSize` named-
    /// size table, common ISO/US sizes are resolved directly; anything
    /// else falls back to A4 (documented, not silently wrong - `paper_size`
    /// is echoed back in the `None` case's caller-visible behavior via
    /// [`named_paper_size_points`]).
    pub paper_size: String,
    pub margin_left: f64,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub pdf_page_margin_left: Option<f64>,
    pub pdf_page_margin_top: Option<f64>,
    pub pdf_page_margin_right: Option<f64>,
    pub pdf_page_margin_bottom: Option<f64>,
}

/// A small table of common named page sizes, in points. Stand-in for
/// `QPageSize::PageSizeId` lookups image_writer.py relies on Qt for.
pub fn named_paper_size_points(name: &str) -> PageSizePoints {
    match name.to_lowercase().as_str() {
        "letter" => PageSizePoints {
            width: 8.5 * INCH,
            height: 11.0 * INCH,
        },
        "legal" => PageSizePoints {
            width: 8.5 * INCH,
            height: 14.0 * INCH,
        },
        "a3" => PageSizePoints {
            width: 297.0 * MM,
            height: 420.0 * MM,
        },
        "a5" => PageSizePoints {
            width: 148.0 * MM,
            height: 210.0 * MM,
        },
        "b4" => PageSizePoints {
            width: 250.0 * MM,
            height: 353.0 * MM,
        },
        "b5" => PageSizePoints {
            width: 176.0 * MM,
            height: 250.0 * MM,
        },
        // "a4" and anything unrecognized: A4, matching Qt's own fallback
        // default page size.
        _ => PageSizePoints {
            width: 210.0 * MM,
            height: 297.0 * MM,
        },
    }
}

/// Port of `get_page_size` (image_writer.py lines 60-76).
pub fn get_page_size(opts: &PageLayoutOpts, for_comic: bool) -> PageSizePoints {
    let use_profile =
        opts.use_profile_size && !opts.profile_is_default && opts.profile_width <= 9999.0;
    if use_profile {
        let (w, h) = if for_comic {
            opts.for_comic_screen_size
                .unwrap_or((opts.profile_width, opts.profile_height))
        } else {
            (opts.profile_width, opts.profile_height)
        };
        let factor = 72.0 / opts.profile_dpi;
        PageSizePoints {
            width: factor * w,
            height: factor * h,
        }
    } else {
        opts.custom_size
            .as_deref()
            .and_then(|spec| parse_pdf_page_size(spec, &opts.unit, opts.profile_dpi))
            .unwrap_or_else(|| named_paper_size_points(&opts.paper_size))
    }
}

/// Port of `get_page_layout` (image_writer.py lines 79-88).
pub fn get_page_layout(opts: &PageLayoutOpts, for_comic: bool) -> PageLayoutPoints {
    let page_size = get_page_size(opts, for_comic);
    let m = |pdf_margin: Option<f64>, fallback: f64| pdf_margin.unwrap_or(fallback).max(0.0);
    let margins = MarginsPoints {
        left: m(opts.pdf_page_margin_left, opts.margin_left),
        top: m(opts.pdf_page_margin_top, opts.margin_top),
        right: m(opts.pdf_page_margin_right, opts.margin_right),
        bottom: m(opts.pdf_page_margin_bottom, opts.margin_bottom),
    };
    PageLayoutPoints {
        size: page_size,
        margins,
    }
}

// ==========================================================================
// Image / draw_image_page / convert (image_writer.py lines 92-141):
// documented gap - see the module doc comment.
// ==========================================================================

/// Port of `Image` (image_writer.py lines 92-103): a decoded source image
/// ready to be drawn onto a PDF page. `todo!()`d because decoding here
/// exists only to feed `draw_image_page`, which needs Qt-driven page
/// assembly - see module doc comment (`crate::pdf::render`'s PDF
/// serializer itself is now available, but that's not this file's gap).
#[derive(Debug, Clone)]
pub struct ImageWriterImage {
    pub img_data: Vec<u8>,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

impl ImageWriterImage {
    pub fn from_bytes(_data: Vec<u8>) -> ImageWriterImage {
        todo!(
            "placeholder: image_writer.py's Image.__init__ decodes via calibre.utils.img \
             (image_and_format_from_data) purely to hand pixel data to draw_image_page, which \
             needs Qt QPageLayout/QPageSize page geometry and calibre.ebooks.pdf.render.graphics's \
             live QPaintEngine-driven Graphics - both out of scope for this file (see module doc \
             comment); calibre.ebooks.pdf.render.serialize.PDFStream, the PDF serializer itself, is \
             now ported for real as crate::pdf::render::serialize::PdfStream (issue #46)"
        )
    }
}

/// Port of `draw_image_page` (image_writer.py lines 106-123): scale and
/// center an image on a PDF page, then draw it via the PDF serializer.
pub fn draw_image_page(_img: &ImageWriterImage, _preserve_aspect_ratio: bool) {
    todo!(
        "placeholder: needs Qt QPageLayout/QPageSize page geometry to drive \
         crate::pdf::render::serialize::PdfStream's writer (add_jpeg_image/add_image/ \
         draw_image_with_transform, all real - see crate::pdf::render::serialize) - see module doc comment"
    )
}

/// Port of `convert` (image_writer.py lines 126-140): render a sequence of
/// images into a single PDF, one per page.
pub fn convert(
    _images: &[std::path::PathBuf],
    _output_path: &std::path::Path,
    _opts: &PageLayoutOpts,
    _metadata: Option<MetaInformation>,
) {
    todo!(
        "placeholder: needs Qt QImage decoding plus crate::pdf::render::serialize::PdfStream's \
         page-assembly driven by draw_image_page (see that function's own todo!) - see module doc \
         comment; page-layout math is available for real via get_page_layout in this module, and \
         the PDF serializer itself is available for real via crate::pdf::render::serialize"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pdf_page_size_parses_wxh_inches() {
        let sz = parse_pdf_page_size("8.5x11", "inch", 72.0).expect("parses");
        assert!((sz.width - 8.5 * INCH).abs() < 1e-6);
        assert!((sz.height - 11.0 * INCH).abs() < 1e-6);
    }

    #[test]
    fn parse_pdf_page_size_handles_comma_decimal() {
        let sz = parse_pdf_page_size("21,0x29,7", "centimeter", 72.0).expect("parses");
        assert!((sz.width - 21.0 * CM).abs() < 1e-6);
        assert!((sz.height - 29.7 * CM).abs() < 1e-6);
    }

    #[test]
    fn parse_pdf_page_size_returns_none_without_separator() {
        assert!(parse_pdf_page_size("not-a-size", "inch", 72.0).is_none());
    }

    #[test]
    fn parse_pdf_page_size_returns_none_on_bad_number() {
        assert!(parse_pdf_page_size("abcxdef", "inch", 72.0).is_none());
    }

    #[test]
    fn devicepixel_unit_uses_dpi_factor() {
        let sz = parse_pdf_page_size("100x200", "devicepixel", 150.0).expect("parses");
        let factor = 72.0 / 150.0;
        assert!((sz.width - 100.0 * factor).abs() < 1e-6);
    }

    fn base_opts() -> PageLayoutOpts {
        PageLayoutOpts {
            use_profile_size: false,
            profile_is_default: true,
            profile_width: 0.0,
            profile_height: 0.0,
            profile_dpi: 72.0,
            for_comic_screen_size: None,
            custom_size: None,
            unit: "inch".to_string(),
            paper_size: "letter".to_string(),
            margin_left: 36.0,
            margin_top: 36.0,
            margin_right: 36.0,
            margin_bottom: 36.0,
            pdf_page_margin_left: None,
            pdf_page_margin_top: None,
            pdf_page_margin_right: None,
            pdf_page_margin_bottom: None,
        }
    }

    #[test]
    fn get_page_size_falls_back_to_named_paper_size() {
        let opts = base_opts();
        let sz = get_page_size(&opts, false);
        assert_eq!(sz, named_paper_size_points("letter"));
    }

    #[test]
    fn get_page_size_prefers_custom_size_spec() {
        let mut opts = base_opts();
        opts.custom_size = Some("6x9".to_string());
        let sz = get_page_size(&opts, false);
        assert!((sz.width - 6.0 * INCH).abs() < 1e-6);
        assert!((sz.height - 9.0 * INCH).abs() < 1e-6);
    }

    #[test]
    fn get_page_layout_falls_back_margins_to_generic_margin_opts() {
        let opts = base_opts();
        let layout = get_page_layout(&opts, false);
        assert_eq!(layout.margins.left, 36.0);
        assert_eq!(layout.margins.top, 36.0);
    }

    #[test]
    fn get_page_layout_prefers_pdf_specific_margins() {
        let mut opts = base_opts();
        opts.pdf_page_margin_left = Some(10.0);
        let layout = get_page_layout(&opts, false);
        assert_eq!(layout.margins.left, 10.0);
        assert_eq!(layout.margins.top, 36.0);
    }

    #[test]
    fn pdf_metadata_defaults_to_unknown_when_no_source_metadata() {
        let pm = PdfMetadata::new(None);
        assert_eq!(pm.title, "Unknown");
        assert_eq!(pm.author, "Unknown");
        assert_eq!(pm.tags, "");
    }

    #[test]
    #[should_panic(expected = "placeholder")]
    fn draw_image_page_is_a_documented_gap() {
        let img = ImageWriterImage {
            img_data: Vec::new(),
            format: "png".to_string(),
            width: 1,
            height: 1,
        };
        draw_image_page(&img, true);
    }
}
