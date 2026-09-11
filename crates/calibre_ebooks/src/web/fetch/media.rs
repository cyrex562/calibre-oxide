//! Port of `RecursiveFetcher.process_images`/`process_stylesheets`
//! (`old_src/src/calibre/web/fetch/simple.py`, issue #632, split from
//! the #455 epic -- see `docs/modules_to_port.md`'s `simple.py` entry
//! for the full split rationale): per-page image and CSS download/
//! rewrite orchestration.
//!
//! All the hard numeric/format work is already real elsewhere in this
//! crate -- this module's own job is purely the fetch -> cache-check
//! -> type-detect -> optionally-compress/re-encode -> write -> rewrite
//! orchestration:
//! - Fetching goes through a caller-supplied closure matching
//!   [`crate::web::fetch::simple::fetch_url`]'s own shape (kept as an
//!   injected closure, not a hard dependency on `Browser`/
//!   `FetchThrottle` directly, so this module stays testable against a
//!   stub fetcher without a real network).
//! - Format detection is [`calibre_utils::imghdr::what`].
//! - Compression is [`crate::web::fetch::utils::rescale_image`] (#83).
//! - Filename sanitizing is [`calibre_utils::filenames::ascii_filename`].
//!
//! Dedup caches (`image_cache`/`stylesheet_cache`, real Python's
//! `self.imagemap`/`self.stylemap`) are taken as `&Mutex<HashMap<...>>`
//! rather than owned privately -- real `RecursiveFetcher` instances
//! share the *same* maps across sibling article fetchers (#633's job
//! to actually wire that sharing up); this module only needs them to
//! already exist and be lockable.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use regex::Regex;

use crate::dom::{Dom, NodeId, NodeKind};
use crate::web::fetch::simple::{FetchError, FetchedResource};
use crate::web::fetch::utils::rescale_image;

fn absolutize(base_url: &str, href: &str) -> String {
    match url::Url::parse(href) {
        Ok(_) => href.to_string(),
        Err(_) => url::Url::parse(base_url).ok().and_then(|b| b.join(href).ok()).map(|u| u.to_string()).unwrap_or_else(|| href.to_string()),
    }
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

// ===================================================================
// Images
// ===================================================================

/// Port of the `compress_news_images`/`compress_news_images_max_size`/
/// `compress_news_images_auto_size`/`scale_news_images` config quartet
/// [`process_images`] needs -- bundled to keep that function's own
/// parameter list manageable.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageCompressionOptions {
    pub compress_news_images: bool,
    pub compress_news_images_max_size_kb: Option<u32>,
    pub compress_news_images_auto_size: u32,
    pub scale_news_images: Option<(u32, u32)>,
}

/// Port of `RecursiveFetcher.rescale_image` + the re-encode branch of
/// `process_images` (`image_from_data`/`image_to_data`). Returns
/// `None` if `data` doesn't decode as an image at all -- matching real
/// Python's own broad `except Exception: continue` around this step.
fn reencode_and_compress(data: &[u8], detected: Option<&str>, compression: &ImageCompressionOptions) -> Option<(Vec<u8>, String)> {
    // Port of `img = image_from_data(data)`: validates decodability
    // even when no re-encode is needed below.
    image::load_from_memory(data).ok()?;

    let (mut out_data, mut itype): (Vec<u8>, String) = match detected {
        Some(t) if matches!(t, "png" | "jpg" | "jpeg") => (data.to_vec(), t.to_string()),
        Some("gif") => (encode_png(data), "png".to_string()),
        _ => (crate::web::fetch::utils::encode_jpeg(&image::load_from_memory(data).ok()?, 95), "jpeg".to_string()),
    };

    if compression.compress_news_images {
        out_data = rescale_image(&out_data, compression.scale_news_images, compression.compress_news_images_max_size_kb, Some(compression.compress_news_images_auto_size as f64));
    }
    // Port of `# Moon+ apparently cannot handle .jpeg files`.
    if itype == "jpeg" {
        itype = "jpg".to_string();
    }
    Some((out_data, itype))
}

fn encode_png(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Ok(img) = image::load_from_memory(data) {
        let _ = img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    }
    buf
}

/// Port of `RecursiveFetcher.process_images`. `fetch` matches
/// [`crate::web::fetch::simple::fetch_url`]'s own signature shape
/// (rate limiting, `file:`/`data:` handling, retry) -- callers pass a
/// closure closing over a real `Browser`/`FetchThrottle` (or a stub,
/// in tests). `image_url_processor`/`preprocess_image` match
/// [`crate::web::feeds::recipe::NewsRecipeHooks`]'s already-real hook
/// shapes exactly (kept as closures here to avoid a hard dependency
/// on that trait from this module).
///
/// Disclosed simplification, not a narrowing: real Python's `data:`
/// URL branch inside `process_images` calls raw `urlopen(iurl).read()`
/// directly rather than `self.fetch_url` (which *also* has its own,
/// separate `data:` handling) -- two code paths that decode to the
/// same bytes. This port uses `fetch` uniformly for every branch,
/// including `data:` URLs, since #630's `fetch_url` already handles
/// them identically.
#[allow(clippy::too_many_arguments)]
pub fn process_images(dom: &mut Dom, base_url: &str, disk_dir: &Path, image_cache: &Mutex<HashMap<String, String>>, fetch: impl Fn(&str) -> Result<FetchedResource, FetchError>, image_url_processor: impl Fn(&str, &str) -> Option<String>, preprocess_image: impl Fn(Vec<u8>, &str) -> Option<Vec<u8>>, compression: &ImageCompressionOptions) -> std::io::Result<()> {
    std::fs::create_dir_all(disk_dir)?;
    let mut counter = 0u32;

    for img in dom.find_all_tag_global("img") {
        let Some(src) = dom.node(img).attrs.get("src").cloned() else { continue };

        let (data, cache_key) = if let Some(payload) = src.strip_prefix("data:") {
            let _ = payload;
            match fetch(&src) {
                Ok(r) => (r.data, None),
                Err(_) => continue,
            }
        } else {
            let Some(processed) = image_url_processor(base_url, &src) else { continue };
            let iurl = absolutize(base_url, &processed);
            if let Some(cached) = image_cache.lock().unwrap().get(&iurl).cloned() {
                dom.node_mut(img).attrs.insert("src".to_string(), cached);
                continue;
            }
            match fetch(&iurl) {
                Ok(r) => {
                    // Port of the literal `b'GIF89a\x01'` tracking-pixel
                    // skip -- a 1x1 empty GIF that PIL/`image` would
                    // error on anyway.
                    if r.data == b"GIF89a\x01" {
                        continue;
                    }
                    (r.data, Some(iurl))
                }
                Err(_) => continue,
            }
        };

        counter += 1;
        let fname = calibre_utils::filenames::ascii_filename(&format!("img{counter}"));
        let Some(data) = preprocess_image(data, &src) else { continue };

        let detected = calibre_utils::imghdr::what(&data);
        let is_svg = detected == Some("svg") || (detected.is_none() && contains_subslice(&data[..data.len().min(1024)], b"<svg"));

        let (out_data, ext) = if is_svg {
            (data, "svg".to_string())
        } else {
            match reencode_and_compress(&data, detected, compression) {
                Some(result) => result,
                None => continue,
            }
        };

        let imgpath = disk_dir.join(format!("{fname}.{ext}"));
        std::fs::write(&imgpath, &out_data)?;
        let path_str = imgpath.to_string_lossy().into_owned();
        if let Some(key) = cache_key {
            image_cache.lock().unwrap().insert(key, path_str.clone());
        }
        dom.node_mut(img).attrs.insert("src".to_string(), path_str);
    }
    Ok(())
}

// ===================================================================
// Stylesheets
// ===================================================================

fn css_import_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)@import\s+url\((.*?)\)").expect("static pattern"))
}

fn collect_text_descendants(dom: &Dom, id: NodeId, out: &mut Vec<NodeId>) {
    for &c in &dom.node(id).children {
        if matches!(dom.node(c).kind, NodeKind::Text(_)) {
            out.push(c);
        } else {
            collect_text_descendants(dom, c, out);
        }
    }
}

/// Port of `RecursiveFetcher.process_stylesheets`. See
/// [`process_images`]'s own doc for `fetch`'s shape.
pub fn process_stylesheets(dom: &mut Dom, base_url: &str, disk_dir: &Path, stylesheet_cache: &Mutex<HashMap<String, String>>, fetch: impl Fn(&str) -> Result<FetchedResource, FetchError>) -> std::io::Result<()> {
    std::fs::create_dir_all(disk_dir)?;

    let tags: Vec<NodeId> = dom.preorder_elements(dom.root).into_iter().filter(|&n| matches!(dom.tag(n), Some("link") | Some("style"))).collect();

    for (idx, tag) in tags.into_iter().enumerate() {
        let mtype = dom.node(tag).attrs.get("type").cloned().unwrap_or_else(|| if dom.tag(tag) == Some("style") { "text/css".to_string() } else { String::new() });
        if !mtype.eq_ignore_ascii_case("text/css") {
            continue;
        }

        if let Some(href) = dom.node(tag).attrs.get("href").cloned() {
            let iurl = absolutize(base_url, &href);
            if let Some(cached) = stylesheet_cache.lock().unwrap().get(&iurl).cloned() {
                dom.node_mut(tag).attrs.insert("href".to_string(), cached);
                continue;
            }
            let Ok(resource) = fetch(&iurl) else { continue };
            let stylepath = disk_dir.join(format!("style{idx}.css"));
            std::fs::write(&stylepath, &resource.data)?;
            let path_str = stylepath.to_string_lossy().into_owned();
            stylesheet_cache.lock().unwrap().insert(iurl, path_str.clone());
            dom.node_mut(tag).attrs.insert("href".to_string(), path_str);
        } else {
            // Real Python's own `c` counter here starts from the outer
            // `enumerate` index and is incremented per `@import` found
            // *within this one tag* -- it resets to the next outer
            // index for the next `link`/`style` tag, not continuing
            // from wherever this inner loop left off. Preserved
            // verbatim, including that quirk.
            let mut counter = idx;
            let mut text_nodes = Vec::new();
            collect_text_descendants(dom, tag, &mut text_nodes);
            for text_node in text_nodes {
                let NodeKind::Text(src) = dom.node(text_node).kind.clone() else { continue };
                let Some(caps) = css_import_regex().captures(&src) else { continue };
                let matched_url = caps.get(1).unwrap().as_str().to_string();
                let iurl = absolutize(base_url, &matched_url);

                let cached = stylesheet_cache.lock().unwrap().get(&iurl).cloned();
                if let Some(cached_path) = cached {
                    dom.node_mut(text_node).kind = NodeKind::Text(src.replace(&matched_url, &cached_path));
                    continue;
                }

                let Ok(resource) = fetch(&iurl) else { continue };
                counter += 1;
                let stylepath = disk_dir.join(format!("style{counter}.css"));
                std::fs::write(&stylepath, &resource.data)?;
                let path_str = stylepath.to_string_lossy().into_owned();
                stylesheet_cache.lock().unwrap().insert(iurl, path_str.clone());
                dom.node_mut(text_node).kind = NodeKind::Text(src.replace(&matched_url, &path_str));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_fetch(responses: HashMap<String, Vec<u8>>) -> impl Fn(&str) -> Result<FetchedResource, FetchError> {
        move |url| responses.get(url).cloned().map(|data| FetchedResource { data, new_url: Some(url.to_string()) }).ok_or_else(|| FetchError::Http("not found".to_string()))
    }

    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([1, 2, 3]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
        buf
    }

    fn make_gif(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([9, 9, 9]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Gif).unwrap();
        buf
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("calibre-oxide-test-media-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ===============================================================
    // process_images
    // ===============================================================

    #[test]
    fn process_images_downloads_and_rewrites_a_relative_src() {
        let dir = test_dir("images-basic");
        let mut dom = Dom::parse(r#"<html><body><img src="pic.png"></body></html>"#);
        let png = make_png(4, 4);
        let responses = HashMap::from([("http://example.com/pic.png".to_string(), png)]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses), |_, u| Some(u.to_string()), |d, _| Some(d), &ImageCompressionOptions::default()).unwrap();

        let img = dom.find_first_tag_global("img").unwrap();
        let new_src = dom.node(img).attrs.get("src").unwrap();
        assert!(new_src.ends_with("img1.png"), "{new_src}");
        assert!(std::path::Path::new(new_src).is_file());
        assert_eq!(cache.lock().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_reencodes_a_gif_as_png() {
        let dir = test_dir("images-gif");
        let mut dom = Dom::parse(r#"<html><body><img src="pic.gif"></body></html>"#);
        let gif = make_gif(4, 4);
        let responses = HashMap::from([("http://example.com/pic.gif".to_string(), gif)]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses), |_, u| Some(u.to_string()), |d, _| Some(d), &ImageCompressionOptions::default()).unwrap();

        let img = dom.find_first_tag_global("img").unwrap();
        let new_src = dom.node(img).attrs.get("src").unwrap();
        assert!(new_src.ends_with("img1.png"), "gif should be re-encoded to png: {new_src}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_uses_the_dedup_cache_on_a_second_reference() {
        let dir = test_dir("images-cache");
        let mut dom = Dom::parse(r#"<html><body><img src="pic.png"><img src="pic.png"></body></html>"#);
        let png = make_png(2, 2);
        let responses = HashMap::from([("http://example.com/pic.png".to_string(), png)]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses), |_, u| Some(u.to_string()), |d, _| Some(d), &ImageCompressionOptions::default()).unwrap();

        let imgs = dom.find_all_tag_global("img");
        let src0 = dom.node(imgs[0]).attrs.get("src").unwrap().clone();
        let src1 = dom.node(imgs[1]).attrs.get("src").unwrap().clone();
        assert_eq!(src0, src1, "both references to the same URL should resolve to the same cached local path");
        assert_eq!(cache.lock().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_skips_the_tracking_pixel_signature() {
        let dir = test_dir("images-trackingpixel");
        let mut dom = Dom::parse(r#"<html><body><img src="pixel.gif"></body></html>"#);
        let responses = HashMap::from([("http://example.com/pixel.gif".to_string(), b"GIF89a\x01".to_vec())]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses), |_, u| Some(u.to_string()), |d, _| Some(d), &ImageCompressionOptions::default()).unwrap();

        let img = dom.find_first_tag_global("img").unwrap();
        assert_eq!(dom.node(img).attrs.get("src").map(String::as_str), Some("pixel.gif"), "src is left untouched when the fetch is skipped");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_skips_when_the_url_processor_rejects_the_image() {
        let dir = test_dir("images-rejected");
        let mut dom = Dom::parse(r#"<html><body><img src="ad.png"></body></html>"#);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(HashMap::new()), |_, _| None, |d, _| Some(d), &ImageCompressionOptions::default()).unwrap();

        assert!(cache.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_images_skips_when_preprocess_image_rejects_it() {
        let dir = test_dir("images-preprocess-reject");
        let mut dom = Dom::parse(r#"<html><body><img src="pic.png"></body></html>"#);
        let png = make_png(2, 2);
        let responses = HashMap::from([("http://example.com/pic.png".to_string(), png)]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("images");

        process_images(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses), |_, u| Some(u.to_string()), |_, _| None, &ImageCompressionOptions::default()).unwrap();

        let img = dom.find_first_tag_global("img").unwrap();
        assert_eq!(dom.node(img).attrs.get("src").map(String::as_str), Some("pic.png"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ===============================================================
    // process_stylesheets
    // ===============================================================

    #[test]
    fn process_stylesheets_downloads_a_linked_stylesheet() {
        let dir = test_dir("css-link");
        let mut dom = Dom::parse(r#"<html><head><link rel="stylesheet" type="text/css" href="style.css"></head><body></body></html>"#);
        let responses = HashMap::from([("http://example.com/style.css".to_string(), b"body { color: red; }".to_vec())]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("stylesheets");

        process_stylesheets(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses)).unwrap();

        let link = dom.find_first_tag_global("link").unwrap();
        let href = dom.node(link).attrs.get("href").unwrap();
        assert!(std::path::Path::new(href).is_file());
        assert_eq!(std::fs::read_to_string(href).unwrap(), "body { color: red; }");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_stylesheets_skips_non_css_link_tags() {
        let dir = test_dir("css-skip-noncss");
        let mut dom = Dom::parse(r#"<html><head><link rel="icon" type="image/png" href="favicon.png"></head><body></body></html>"#);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("stylesheets");

        process_stylesheets(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(HashMap::new())).unwrap();

        let link = dom.find_first_tag_global("link").unwrap();
        assert_eq!(dom.node(link).attrs.get("href").map(String::as_str), Some("favicon.png"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_stylesheets_rewrites_an_at_import_inside_an_inline_style_tag() {
        let dir = test_dir("css-import");
        let mut dom = Dom::parse("<html><head><style type=\"text/css\">@import url(fonts.css);</style></head><body></body></html>");
        let responses = HashMap::from([("http://example.com/fonts.css".to_string(), b"@font-face {}".to_vec())]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("stylesheets");

        process_stylesheets(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses)).unwrap();

        let style = dom.find_first_tag_global("style").unwrap();
        let text = dom.text_content(style);
        assert!(!text.contains("fonts.css"), "{text}");
        assert!(text.contains("style1.css") || text.contains("style0.css"), "{text}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn process_stylesheets_uses_the_dedup_cache_for_a_repeated_href() {
        let dir = test_dir("css-cache");
        let mut dom = Dom::parse(r#"<html><head><link type="text/css" href="a.css"><link type="text/css" href="a.css"></head><body></body></html>"#);
        let responses = HashMap::from([("http://example.com/a.css".to_string(), b"x{}".to_vec())]);
        let cache = Mutex::new(HashMap::new());
        let disk_dir = dir.join("stylesheets");

        process_stylesheets(&mut dom, "http://example.com/", &disk_dir, &cache, stub_fetch(responses)).unwrap();

        let links = dom.find_all_tag_global("link");
        let href0 = dom.node(links[0]).attrs.get("href").unwrap().clone();
        let href1 = dom.node(links[1]).attrs.get("href").unwrap().clone();
        assert_eq!(href0, href1);
        assert_eq!(cache.lock().unwrap().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
