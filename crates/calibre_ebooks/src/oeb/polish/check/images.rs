//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/images.py`.
//!
//! # Real: decode-and-inspect
//!
//! [`check_raster_images`] is fully real: "does this image decode
//! without error" uses the `image` crate (this crate's now-added,
//! `docs/AGENT_PORTING_GUIDE.md` §6-named dependency for image work,
//! narrowly scoped here to decoding+inspection only, no re-encoding).
//! "Is this JPEG's colorspace CMYK" specifically needs the `jpeg-decoder`
//! crate directly (a transitive dependency of `image` already, added
//! here as a direct one too) rather than `image`'s own `DynamicImage`
//! API: `image` 0.24's JPEG decoder *silently converts* CMYK data to
//! RGB before a caller ever sees a `ColorType` (see
//! `image::codecs::jpeg::JpegDecoder::new`, which overwrites its cached
//! `pixel_format` from `CMYK32` to `RGB24` at construction time, and
//! whose `color_type()` would otherwise panic on a real `CMYK32`).
//! `jpeg_decoder::Decoder::info()` still reports the true, pre-conversion
//! pixel format, which is what `check_raster_images` actually needs to
//! know -- matching Python's `PIL.Image.open(...).mode == 'CMYK'`, which
//! reports the source colorspace rather than silently converting it.
//!
//! # Gap: `CMYKImage`'s auto-fix
//!
//! Python's `CMYKImage.__call__` converts the image to RGB via Qt's
//! `QImage.loadFromData`/`calibre.gui2.pixmap_to_data` -- a GUI-toolkit
//! round-trip, not decode+inspect. This crate has no Qt dependency (see
//! `docs/AGENT_PORTING_GUIDE.md` §4's GUI guidance: `iced`, not Qt) and
//! adding one for a single auto-fix button is out of scope. Real
//! CMYK-to-RGB *conversion* (as opposed to detection) would need either
//! a Qt embed or `image`-crate-based re-encoding logic equivalent to
//! `oeb::polish::images`' already-`todo!()` recompression gap (issue
//! #162) -- [`CMYKImage`]'s fix is `todo!()` for the same reason.

use std::io::Cursor;

use anyhow::Result;

use super::base::{CheckError, Level};
use super::parsing::empty_file;

/// Port of `InvalidImage`.
pub fn invalid_image(msg: &str, name: &str) -> CheckError {
    CheckError::new("InvalidImage", format!("Invalid image: {msg}"), name).with_help(
        "An invalid image is an image that could not be loaded, typically because it is \
         corrupted. You should replace it with a good image or remove it.",
    )
}

/// Port of `CMYKImage`. See the module docs for why its auto-fix is
/// `todo!()`.
pub fn cmyk_image(name: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new("CMYKImage", "Image is in the CMYK colorspace", name)
        .with_level(Level::Warn)
        .with_help(
            "Reader devices based on Adobe Digital Editions cannot display images whose \
             colors are specified in the CMYK colorspace. You should convert this image to \
             the RGB colorspace, for maximum compatibility.",
        )
        .with_fix("Convert image to RGB automatically", move |_container| {
            todo!(
                "placeholder: CMYKImage's auto-fix converts CMYK -> RGB via Qt's QImage/ \
                 pixmap_to_data in Python (calibre.gui2.pixmap_to_data) -- this crate has no \
                 Qt dependency (image {owned_name} would need real re-encoding, the same \
                 documented gap as oeb::polish::images' recompression, issue #162)"
            )
        })
}

/// Returns `Ok(true)` if `raw` is a CMYK-colorspace JPEG, `Ok(false)` if
/// it decodes fine and isn't, or `Err` if it does not decode at all.
/// Port of the decode-and-inspect half of `check_raster_images`.
fn is_cmyk_jpeg_or_decode_error(raw: &[u8]) -> Result<bool, String> {
    let format = image::guess_format(raw).map_err(|e| e.to_string())?;
    if format == image::ImageFormat::Jpeg {
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(raw));
        decoder.read_info().map_err(|e| e.to_string())?;
        let info = decoder
            .info()
            .ok_or_else(|| "could not read JPEG header".to_string())?;
        Ok(info.pixel_format == jpeg_decoder::PixelFormat::CMYK32)
    } else {
        image::load_from_memory_with_format(raw, format).map_err(|e| e.to_string())?;
        Ok(false)
    }
}

/// Port of `check_raster_images`.
pub fn check_raster_images(name: &str, raw: &[u8]) -> Vec<CheckError> {
    if raw.is_empty() {
        return vec![empty_file(name)];
    }
    match is_cmyk_jpeg_or_decode_error(raw) {
        Ok(true) => vec![cmyk_image(name)],
        Ok(false) => Vec::new(),
        Err(msg) => vec![invalid_image(&msg, name)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real, freshly-encoded 2x2 RGB PNG (rather than a hand-written
    /// byte literal, which is fragile to get exactly CRC-correct by
    /// hand).
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageOutputFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn check_raster_images_flags_empty_file() {
        let errors = check_raster_images("a.png", b"");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "EmptyFile");
    }

    #[test]
    fn check_raster_images_flags_corrupt_data() {
        let errors = check_raster_images("a.png", b"not an image at all, just text");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].type_name, "InvalidImage");
    }

    #[test]
    fn check_raster_images_accepts_valid_png() {
        let errors = check_raster_images("a.png", &tiny_png());
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn cmyk_image_fix_is_a_documented_gap() {
        let mut err = cmyk_image("a.jpg");
        assert!(err.is_fixable());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest/><spine/>
</package>"#,
        )
        .unwrap();
        let mut c = super::super::super::container::Container::open(
            dir.path(),
            &dir.path().join("content.opf"),
        )
        .unwrap();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| err.apply_fix(&mut c)));
        assert!(
            result.is_err(),
            "expected the documented todo!() gap to panic"
        );
    }
}
