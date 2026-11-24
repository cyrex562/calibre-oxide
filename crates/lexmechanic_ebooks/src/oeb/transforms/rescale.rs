//! Port of `old_src/src/calibre/ebooks/oeb/transforms/rescale.py`.

use image::{GenericImageView, ImageFormat};

use crate::oeb::book::OEBBook;

/// Port of `calibre.fit_image`: fit an image of `width`x`height` inside
/// a `pwidth`x`pheight` box, preserving aspect ratio. Returns `(scaled,
/// new_width, new_height)` where `scaled` is true iff the dimensions
/// changed.
pub fn fit_image(width: f64, height: f64, pwidth: f64, pheight: f64) -> (bool, i64, i64) {
    if height < 1.0 || width < 1.0 {
        return (false, width as i64, height as i64);
    }
    let mut width = width;
    let mut height = height;
    let scaled = height > pheight || width > pwidth;
    if height > pheight {
        let corrf = pheight / height;
        width = (corrf * width).floor();
        height = pheight;
    }
    if width > pwidth {
        let corrf = pwidth / width;
        width = pwidth;
        height = (corrf * height).floor();
    }
    if height > pheight {
        let corrf = pheight / height;
        width = (corrf * width).floor();
        height = pheight;
    }
    (scaled, width as i64, height as i64)
}

/// Options this transform reads from the destination output profile /
/// conversion options (`opts.dest.*`, `opts.margin_*`,
/// `opts.is_image_collection`). No shared conversion-options type exists
/// yet -- see other transforms' options structs for the same narrowing.
#[derive(Debug, Clone, Default)]
pub struct RescaleOptions {
    pub is_image_collection: bool,
    pub comic_screen_size: Option<(f64, f64)>,
    pub dest_width: f64,
    pub dest_height: f64,
    pub dest_dpi: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
}

/// Port of `RescaleImages`: shrink every raster image to fit the target
/// device's page box.
#[derive(Default)]
pub struct RescaleImages {
    pub check_colorspaces: bool,
}

impl RescaleImages {
    /// `max_size` mirrors Python's `max_size: str = 'profile'` parameter:
    /// `"profile"` uses `opts`'s page box, `"none"` disables rescaling,
    /// and `"<width>x<height>"` (e.g. `"600x800"`) overrides it.
    pub fn call(&self, oeb: &mut OEBBook, opts: &RescaleOptions, max_size: &str) {
        let no_scale_size = 99_999_999_999.0f64;
        let (mut page_width, mut page_height) = if opts.is_image_collection {
            opts.comic_screen_size
                .unwrap_or((no_scale_size, no_scale_size))
        } else {
            let w = opts.dest_width - (opts.margin_left + opts.margin_right) * opts.dest_dpi / 72.0;
            let h =
                opts.dest_height - (opts.margin_top + opts.margin_bottom) * opts.dest_dpi / 72.0;
            (w, h)
        };

        if max_size == "none" {
            page_width = no_scale_size;
            page_height = no_scale_size;
        } else if max_size != "profile" {
            let s = max_size.trim().to_lowercase();
            let (w, h) = s.split_once('x').unwrap_or((s.as_str(), ""));
            page_width = w
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| *v > 0.0)
                .unwrap_or(no_scale_size);
            page_height = h
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| *v > 0.0)
                .unwrap_or(no_scale_size);
        }

        let items: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .filter(|i| i.media_type.starts_with("image"))
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in items {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            if raw.is_empty() {
                continue;
            }
            let Ok(img) = image::load_from_memory(&raw) else {
                // Probably an SVG or otherwise undecodable image.
                continue;
            };
            let (width, height) = img.dimensions();
            // `self.check_colorspaces` (Python: convert CMYK to RGB
            // before resizing, since Adobe Digital Editions can't
            // display CMYK) has no effect here: this crate's `image`
            // dependency decodes JPEG/PNG/GIF straight to RGB(A) without
            // exposing a CMYK variant, so there is never a CMYK image to
            // convert by the time it reaches this loop.
            let mut img = img;
            let (scaled, new_width, new_height) =
                fit_image(width as f64, height as f64, page_width, page_height);
            if scaled {
                let new_width = new_width.max(1) as u32;
                let new_height = new_height.max(1) as u32;
                img =
                    img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);
                let fmt = guess_format(&media_type);
                let mut buf = std::io::Cursor::new(Vec::new());
                if img.write_to(&mut buf, fmt).is_ok() {
                    let _ = oeb.container.write(&href, buf.get_ref());
                }
            }
        }
    }
}

fn guess_format(media_type: &str) -> ImageFormat {
    match media_type {
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        _ => ImageFormat::Jpeg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn fit_image_shrinks_to_bounding_box() {
        let (scaled, w, h) = fit_image(1000.0, 500.0, 400.0, 400.0);
        assert!(scaled);
        assert!(w <= 400 && h <= 400);
        assert_eq!(w, 400);
        assert_eq!(h, 200);
    }

    #[test]
    fn fit_image_leaves_small_images_alone() {
        let (scaled, w, h) = fit_image(100.0, 50.0, 400.0, 400.0);
        assert!(!scaled);
        assert_eq!((w, h), (100, 50));
    }

    #[test]
    fn rescale_shrinks_an_oversized_png() {
        let img = image::DynamicImage::new_rgb8(200, 100);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let mut oeb = Builder::new()
            .part("big.png", "image/png", buf.get_ref(), false)
            .build();
        let opts = RescaleOptions {
            dest_width: 100.0,
            dest_height: 100.0,
            dest_dpi: 72.0,
            ..RescaleOptions::default()
        };
        RescaleImages::default().call(&mut oeb, &opts, "profile");
        let raw = oeb.container.read("big.png").unwrap();
        let out = image::load_from_memory(&raw).unwrap();
        assert!(out.dimensions().0 <= 100);
        assert!(out.dimensions().1 <= 100);
    }
}
