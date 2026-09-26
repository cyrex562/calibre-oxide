//! Rasterizing PDF pages to images.
//!
//! # What this ports
//!
//! `old_src/src/calibre/ebooks/metadata/pdf.py` renders pages by
//! shelling out to poppler's `pdftoppm`, in two places:
//!
//! ```text
//! read_info(outputdir, get_cover):        # -> cover.jpg
//!     check_call(pdftoppm, '-singlefile', '-jpeg', '-cropbox', 'src.pdf', 'cover')
//!
//! page_images(pdfpath, outputdir, first, last, image_format, prefix):
//!     check_call(pdftoppm, '-cropbox', '-' + image_format, '-f', first, '-l', last, ...)
//! ```
//!
//! The first is why a PDF gets a cover at all in calibre: page 1,
//! rasterized at import time. The second is the general
//! "render this page range to images" primitive.
//!
//! Both are ported here, as [`first_page_cover`] and [`page_images`].
//! What changes is the engine: PDFium rather than poppler. Requiring
//! `poppler-utils` on `PATH` is workable on a Linux distribution and
//! genuinely awkward on Windows, which is where this project's users
//! are; PDFium is a library this can link against on every platform
//! the app ships to.
//!
//! `-cropbox` has no direct equivalent -- PDFium renders the crop box
//! by default, which is the behaviour that flag selects.
//!
//! # Why the library is loaded dynamically
//!
//! PDFium is a C++ library; there is no pure-Rust rasterizer worth
//! using. The binding could be static (link `libpdfium.a` at build
//! time) or dynamic (`dlopen` at run time). This uses dynamic, and
//! the reason is a build-time one rather than a runtime one:
//!
//! - **Static** would make a PDFium binary a prerequisite of
//!   `cargo build`. That is what `ort` does in this workspace, and it
//!   was the single most awkward part of getting the Windows build
//!   working -- there is no prebuilt for `x86_64-pc-windows-gnu`, so
//!   it needs environment variables set before the workspace will
//!   compile at all.
//! - **Dynamic** means `cargo build --workspace` needs nothing:
//!   `libloading` resolves the symbols when a PDF is first rendered.
//!   A checkout with no PDFium present still builds, still tests, and
//!   still runs -- PDF rendering is simply unavailable, which
//!   [`is_available`] reports and callers degrade around.
//!
//! So a missing PDFium is a *runtime* condition here, not a build
//! failure, and that is deliberate. `cargo xtask fetch-pdfium`
//! downloads the platform's library; see `docs/WINDOWS.md`.
//!
//! # Why one process-wide instance
//!
//! PDFium's own documentation requires `FPDF_InitLibrary` /
//! `FPDF_DestroyLibrary` to be called once per process. A `Pdfium`
//! value owns that pair, so creating one per render would init and
//! destroy the library repeatedly. [`pdfium`] holds a single instance
//! in a `OnceLock`; `pdfium-render`'s `thread_safe` feature serialises
//! access to the (not thread-safe) C++ core behind it, which is what
//! makes a shared instance sound to hand out to several server
//! request threads at once.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

/// Width a cover is rendered at. Tall enough that the cover grid and
/// the details panel both have real pixels to downscale from, rather
/// than a thumbnail that looks soft at any useful size.
pub const DEFAULT_COVER_WIDTH: u32 = 1000;

/// Upper bound on a render, so a bad number cannot reach the
/// allocator. A letter page at this width is ~5,200px tall and ~83MB
/// of RGBA -- already well past what a cover or an exported page
/// needs (four times [`DEFAULT_COVER_WIDTH`]), which is the point: it
/// is a backstop, not a useful size.
pub const MAX_RENDER_WIDTH: u32 = 4_000;

/// Matches `save_cover_data_to`'s own default `compression_quality`,
/// which is what covers elsewhere in this crate are encoded at.
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

/// The environment variable that overrides library discovery, holding
/// either the library file itself or a directory containing it.
pub const LIBRARY_PATH_VAR: &str = "CALIBRE_OXIDE_PDFIUM";

#[derive(Debug, thiserror::Error)]
pub enum RasterizeError {
    /// No PDFium library was found. Distinct from every other variant
    /// because it is the one a caller should treat as "this feature is
    /// not installed" rather than "this PDF is broken" -- the server
    /// turns it into a 503 and cover generation skips silently.
    #[error("no PDFium library found (looked for {searched}); run `cargo xtask fetch-pdfium` or set {var}")]
    Unavailable { searched: String, var: &'static str },

    #[error("PDFium could not read this PDF: {0}")]
    Pdf(String),

    #[error("page {page} does not exist (the document has {pages} page(s))")]
    PageOutOfRange { page: usize, pages: usize },

    #[error("could not encode the rendered page: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, RasterizeError>;

/// Image formats a rendered page can be handed back as -- the two
/// `pdftoppm` formats calibre itself uses, and the two a cover is
/// allowed to be (see `calibre_srv`'s `sniff_image_format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageImageFormat {
    Jpeg,
    Png,
}

/// One rasterized page, as straight RGBA rather than an encoded
/// image, so a caller that wants to both encode it *and* inspect it
/// (a blank-page check, say) does not have to decode it back.
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    /// Four bytes per pixel, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl RenderedPage {
    pub fn encode(&self, format: PageImageFormat, jpeg_quality: u8) -> Result<Vec<u8>> {
        use image::{ImageBuffer, Rgba};

        let buffer: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or_else(|| RasterizeError::Encode(format!("{}x{} does not match {} bytes of pixel data", self.width, self.height, self.rgba.len())))?;

        let mut out = Vec::new();
        match format {
            PageImageFormat::Jpeg => {
                // JPEG has no alpha channel, so it has to go. Dropping
                // it (rather than compositing) is correct here because
                // PDFium fills the bitmap with opaque white before
                // drawing -- the alpha is already 255 everywhere.
                let rgb = image::DynamicImage::ImageRgba8(buffer).to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality)
                    .encode(rgb.as_raw(), self.width, self.height, image::ColorType::Rgb8)
                    .map_err(|e| RasterizeError::Encode(e.to_string()))?;
            }
            PageImageFormat::Png => {
                image::codecs::png::PngEncoder::new(&mut out)
                    .encode(buffer.as_raw(), self.width, self.height, image::ColorType::Rgba8)
                    .map_err(|e| RasterizeError::Encode(e.to_string()))?;
            }
        }
        Ok(out)
    }
}

/// Every path PDFium is looked for, in the order tried.
///
/// Split out from the loading itself, and taking its inputs as
/// arguments rather than reading the environment, so the ordering can
/// be tested without a PDFium present or an environment to mutate.
///
/// A directory yields the platform's library name inside it
/// (`libpdfium.so`, `pdfium.dll`, `libpdfium.dylib`); the override may
/// also name the file directly, which is what a developer pointing at
/// a downloaded tarball will naturally do.
pub fn candidate_library_paths(env_override: Option<&OsString>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(raw) = env_override {
        let given = PathBuf::from(raw);
        // Cannot just test `is_file()`: the point of naming the file
        // directly is usually that it is somewhere unusual, and a
        // typo'd path should produce "looked here and did not find
        // it" rather than being silently reinterpreted as a directory.
        if given.extension().is_some() && !given.is_dir() {
            paths.push(given);
        } else {
            paths.push(PathBuf::from(Pdfium::pdfium_platform_library_name_at_path(&given)));
        }
    }

    if let Some(dir) = exe_dir {
        // Next to the binary is where an installed bundle keeps it,
        // and where `cargo xtask fetch-pdfium` puts it for a dev
        // build -- so the same lookup serves both.
        paths.push(PathBuf::from(Pdfium::pdfium_platform_library_name_at_path(dir)));
        if let Some(parent) = dir.parent() {
            // One level up, because a *test* binary does not live
            // where the real binaries do: cargo puts it in
            // `target/debug/deps/`, so the copy in `target/debug/`
            // would be invisible to the very tests that exercise this.
            paths.push(PathBuf::from(Pdfium::pdfium_platform_library_name_at_path(parent)));
            // A Unix install conventionally splits `bin/` and `lib/`.
            paths.push(PathBuf::from(Pdfium::pdfium_platform_library_name_at_path(&parent.join("lib"))));
        }
    }

    paths
}

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

fn searched_paths() -> Vec<PathBuf> {
    candidate_library_paths(std::env::var_os(LIBRARY_PATH_VAR).as_ref(), current_exe_dir().as_deref())
}

/// The process-wide PDFium instance, or `None` if the library is not
/// installed. See this module's docs for why there is exactly one.
///
/// The outcome is cached either way: a missing library stays missing
/// for the life of the process, and repeating a failing `dlopen` on
/// every book during an import of a thousand PDFs would be pure cost.
fn pdfium() -> Option<&'static Pdfium> {
    static INSTANCE: OnceLock<Option<Pdfium>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            for path in searched_paths() {
                if let Ok(bindings) = Pdfium::bind_to_library(&path) {
                    log::debug!("bound PDFium at {}", path.display());
                    return Some(Pdfium::new(bindings));
                }
            }
            // A system-wide install (a distribution package, or
            // `LD_LIBRARY_PATH`) is the last resort rather than the
            // first choice: a bundled copy is the version this was
            // tested against.
            match Pdfium::bind_to_system_library() {
                Ok(bindings) => Some(Pdfium::new(bindings)),
                Err(e) => {
                    log::info!("PDF page rendering is unavailable: {e}");
                    None
                }
            }
        })
        .as_ref()
}

fn unavailable() -> RasterizeError {
    RasterizeError::Unavailable {
        searched: searched_paths().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
        var: LIBRARY_PATH_VAR,
    }
}

/// Whether PDF rendering can be done at all in this process.
///
/// Callers that render *opportunistically* -- cover generation at
/// import time, which must not fail an import -- check this rather
/// than treating [`RasterizeError::Unavailable`] as an error worth
/// reporting.
pub fn is_available() -> bool {
    pdfium().is_some()
}

/// How many pages the document has.
pub fn page_count(pdf: &[u8]) -> Result<usize> {
    let pdfium = pdfium().ok_or_else(unavailable)?;
    let document = pdfium.load_pdf_from_byte_slice(pdf, None).map_err(|e| RasterizeError::Pdf(e.to_string()))?;
    Ok(document.pages().len() as usize)
}

/// Renders one page, scaled so its width is `target_width` pixels.
///
/// `page_number` is 1-based, matching `pdftoppm`'s `-f`/`-l`, the page
/// number printed on the page, and the number the reader's page box
/// shows -- an off-by-one here would silently hand back the wrong
/// page as somebody's cover. `target_width` is clamped to
/// [`MAX_RENDER_WIDTH`].
pub fn render_page(pdf: &[u8], page_number: usize, target_width: u32) -> Result<RenderedPage> {
    let pdfium = pdfium().ok_or_else(unavailable)?;
    let document = pdfium.load_pdf_from_byte_slice(pdf, None).map_err(|e| RasterizeError::Pdf(e.to_string()))?;
    let pages = document.pages();
    let count = pages.len() as usize;

    if page_number < 1 || page_number > count {
        return Err(RasterizeError::PageOutOfRange { page: page_number, pages: count });
    }

    // Infallible after the check above: PDFium counts pages in a
    // `u16`, so `count` -- and therefore `page_number` -- cannot
    // exceed `u16::MAX`.
    let index = (page_number - 1) as u16;
    let page = pages.get(index.into()).map_err(|e| RasterizeError::Pdf(e.to_string()))?;

    let config = PdfRenderConfig::new().set_target_width(target_width.clamp(1, MAX_RENDER_WIDTH) as i32);
    let bitmap = page.render_with_config(&config).map_err(|e| RasterizeError::Pdf(e.to_string()))?;

    Ok(RenderedPage {
        width: bitmap.width() as u32,
        height: bitmap.height() as u32,
        // PDFium's own buffer is BGRA; this converts. Getting it
        // backwards would put out covers with red and blue swapped.
        rgba: bitmap.as_rgba_bytes(),
    })
}

/// Port of `pdf.py`'s `page_images`: renders the inclusive page range
/// `first..=last` and hands back one encoded image per page.
///
/// Unlike the Python, which writes numbered files into a directory,
/// this returns the bytes -- every caller here either serves them over
/// HTTP or stores them as a cover, and neither wants a temporary
/// directory in between.
pub fn page_images(pdf: &[u8], first: usize, last: usize, format: PageImageFormat, target_width: u32) -> Result<Vec<Vec<u8>>> {
    if last < first {
        return Ok(Vec::new());
    }
    (first..=last).map(|page| render_page(pdf, page, target_width)?.encode(format, DEFAULT_JPEG_QUALITY)).collect()
}

/// Port of `read_info`'s `get_cover` branch: page 1 as a JPEG, which
/// is what `mi.cover_data` carries and what `cover.jpg` in a library
/// folder is.
pub fn first_page_cover(pdf: &[u8]) -> Result<Vec<u8>> {
    render_page(pdf, 1, DEFAULT_COVER_WIDTH)?.encode(PageImageFormat::Jpeg, DEFAULT_JPEG_QUALITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real two-page PDF: page 1 draws "PAGE ONE" and a blue
    /// rectangle, page 2 "PAGE TWO" and a red one. The colours are
    /// the point -- they make "did it render the page asked for" and
    /// "are the channels in the right order" both checkable by
    /// looking at a pixel, rather than only that *some* bytes came
    /// back.
    const TWO_PAGE_PDF: &[u8] = include_bytes!("../../tests/data/two-page.pdf");

    /// Skips rather than fails when PDFium is not installed, matching
    /// how the network-storage tests treat an absent NFS server: the
    /// renderer is genuinely optional, and a checkout without it
    /// should still get a green test run.
    fn require_pdfium() -> bool {
        if is_available() {
            return true;
        }
        eprintln!("skipping: no PDFium library available");
        false
    }

    /// Samples the middle of the drawn rectangle, in the page's own
    /// coordinate space converted to image pixels.
    fn rect_pixel(page: &RenderedPage) -> (u8, u8, u8) {
        let scale = page.width as f32 / 612.0;
        let x = (200.0 * scale) as u32;
        // PDF y counts up from the bottom; images count down.
        let y = ((792.0 - 350.0) * scale) as u32;
        let i = ((y * page.width + x) * 4) as usize;
        (page.rgba[i], page.rgba[i + 1], page.rgba[i + 2])
    }

    #[test]
    fn page_count_sees_both_pages() {
        if !require_pdfium() {
            return;
        }
        assert_eq!(page_count(TWO_PAGE_PDF).unwrap(), 2);
    }

    #[test]
    fn rendering_is_rgba_in_that_order() {
        if !require_pdfium() {
            return;
        }
        let page = render_page(TWO_PAGE_PDF, 1, 600).unwrap();
        // Blue, not red: `as_rgba_bytes` really does reorder PDFium's
        // native BGRA.
        assert_eq!(rect_pixel(&page), (0, 0, 255));
    }

    #[test]
    fn page_numbers_are_one_based() {
        if !require_pdfium() {
            return;
        }
        // If these were 0-based, page 1 would come back red.
        assert_eq!(rect_pixel(&render_page(TWO_PAGE_PDF, 1, 600).unwrap()), (0, 0, 255));
        assert_eq!(rect_pixel(&render_page(TWO_PAGE_PDF, 2, 600).unwrap()), (255, 0, 0));
    }

    #[test]
    fn target_width_scales_the_page_and_keeps_its_aspect() {
        if !require_pdfium() {
            return;
        }
        let page = render_page(TWO_PAGE_PDF, 1, 300).unwrap();
        assert_eq!(page.width, 300);
        // 612x792 letter, so a 300px-wide render is ~388 tall.
        assert!((385..=390).contains(&page.height), "unexpected height {}", page.height);
        assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
    }

    #[test]
    fn an_absurd_width_is_clamped_rather_than_attempted() {
        if !require_pdfium() {
            return;
        }
        // Unclamped this asks for a bitmap wider than the address
        // space.
        let page = render_page(TWO_PAGE_PDF, 1, u32::MAX).unwrap();
        assert_eq!(page.width, MAX_RENDER_WIDTH);
    }

    #[test]
    fn a_zero_width_still_renders_something() {
        if !require_pdfium() {
            return;
        }
        assert_eq!(render_page(TWO_PAGE_PDF, 1, 0).unwrap().width, 1);
    }

    #[test]
    fn a_page_past_the_end_is_out_of_range_not_a_pdf_error() {
        if !require_pdfium() {
            return;
        }
        match render_page(TWO_PAGE_PDF, 3, 300) {
            Err(RasterizeError::PageOutOfRange { page, pages }) => {
                assert_eq!((page, pages), (3, 2));
            }
            other => panic!("expected PageOutOfRange, got {other:?}", other = other.map(|_| "a rendered page")),
        }
    }

    #[test]
    fn page_zero_is_rejected_rather_than_wrapping_to_the_last_page() {
        if !require_pdfium() {
            return;
        }
        assert!(matches!(render_page(TWO_PAGE_PDF, 0, 300), Err(RasterizeError::PageOutOfRange { .. })));
    }

    #[test]
    fn jpeg_and_png_encodings_are_both_real_images() {
        if !require_pdfium() {
            return;
        }
        let page = render_page(TWO_PAGE_PDF, 1, 200).unwrap();

        let jpeg = page.encode(PageImageFormat::Jpeg, DEFAULT_JPEG_QUALITY).unwrap();
        assert_eq!(&jpeg[..3], b"\xff\xd8\xff", "not JPEG-framed");
        let png = page.encode(PageImageFormat::Png, DEFAULT_JPEG_QUALITY).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not PNG-framed");

        // Decodable, the right size, and the rectangle survived the
        // round trip -- an encoder that wrote a valid header over
        // garbage would pass the checks above.
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (page.width, page.height));
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        let i = (((page.height / 2) * page.width + page.width / 2) * 4) as usize;
        assert_eq!(decoded.as_raw()[i + 2], 255, "the blue rectangle did not survive PNG encoding");
    }

    #[test]
    fn a_cover_is_a_jpeg_of_page_one_at_cover_width() {
        if !require_pdfium() {
            return;
        }
        let cover = first_page_cover(TWO_PAGE_PDF).unwrap();
        let decoded = image::load_from_memory(&cover).unwrap();
        assert_eq!(decoded.width(), DEFAULT_COVER_WIDTH);
    }

    #[test]
    fn page_images_covers_the_range_inclusively() {
        if !require_pdfium() {
            return;
        }
        assert_eq!(page_images(TWO_PAGE_PDF, 1, 2, PageImageFormat::Png, 100).unwrap().len(), 2);
        assert_eq!(page_images(TWO_PAGE_PDF, 2, 2, PageImageFormat::Png, 100).unwrap().len(), 1);
        // An inverted range is empty, not an error -- `first > last`
        // is how a caller says "nothing".
        assert!(page_images(TWO_PAGE_PDF, 2, 1, PageImageFormat::Png, 100).unwrap().is_empty());
    }

    #[test]
    fn garbage_is_a_pdf_error() {
        if !require_pdfium() {
            return;
        }
        assert!(matches!(render_page(b"this is not a PDF at all", 1, 100), Err(RasterizeError::Pdf(_))));
    }

    // The rest need no PDFium: they are the pure library-discovery
    // logic, which must be testable on a box without it.

    #[test]
    fn the_override_is_tried_before_the_bundled_copy() {
        let paths = candidate_library_paths(Some(&OsString::from("/opt/pdfium/libpdfium.so")), Some(Path::new("/usr/bin")));
        assert_eq!(paths[0], Path::new("/opt/pdfium/libpdfium.so"));
        assert!(paths.len() > 1, "the executable's own directory should still be a fallback");
    }

    #[test]
    fn an_override_naming_a_directory_gets_the_platform_library_name_appended() {
        // `/opt/pdfium` has no extension, so it reads as a directory
        // even though nothing exists at that path.
        let paths = candidate_library_paths(Some(&OsString::from("/opt/pdfium")), None);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].starts_with("/opt/pdfium"));
        assert!(paths[0] != Path::new("/opt/pdfium"), "a bare directory is not loadable");
        assert!(paths[0].file_name().unwrap().to_string_lossy().contains("pdfium"));
    }

    #[test]
    fn the_executables_own_directory_its_parent_and_a_sibling_lib_are_all_searched() {
        let paths = candidate_library_paths(None, Some(Path::new("/opt/calibre-oxide/bin")));
        assert_eq!(paths.len(), 3);
        assert!(paths[0].starts_with("/opt/calibre-oxide/bin"));
        // The parent matters for `target/debug/deps/<test binary>`,
        // which is where every test in this module runs from.
        assert!(paths[1].starts_with("/opt/calibre-oxide/"));
        assert!(paths[2].starts_with("/opt/calibre-oxide/lib"));
    }

    #[test]
    fn with_nothing_to_go_on_there_is_nothing_to_search() {
        // Not an empty *result* by accident: this is the case where
        // only the system library is left to try, and `pdfium()`
        // falls through to it.
        assert!(candidate_library_paths(None, None).is_empty());
    }
}
