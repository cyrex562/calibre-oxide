//! Port of `old_src/src/calibre/ebooks/oeb/polish/images.py`.
//!
//! [`get_compressible_images`] is pure manifest filtering and is ported
//! for real. The `Worker`/`Queue`/`Thread` orchestration in
//! `compress_images` -- scan the manifest, dedupe by resolved path, fan
//! work out across a bounded thread pool, collect
//! `(before_size, after_size)` per file, aggregate and report totals,
//! support early abort via a callback -- is also ported for real, using
//! `std::thread::scope` + a `Mutex`-guarded work queue + an `mpsc`
//! channel for results, the natural Rust equivalent of Python's
//! `Queue`/`Thread`/`Event` trio.
//!
//! What is **not** ported: the actual per-file byte-level recompression
//! (`Worker.compress`, which calls `calibre.utils.img.{encode_jpeg,
//! encode_webp, optimize_jpeg, optimize_png, optimize_webp}`). Those
//! Python functions are backed by Qt's image plugins plus calibre's own
//! multi-pass PNG optimizer -- not just "call a JPEG encoder with a
//! quality setting". `docs/AGENT_PORTING_GUIDE.md` §6 blesses the
//! `image` crate for cover/image processing, but adding it here would
//! only cover the JPEG re-encode case for real; PNG "optimization"
//! specifically implies multi-pass compression (an oxipng-style tool)
//! the `image` crate alone doesn't provide, and quality-controlled WebP
//! encoding needs `libwebp` bindings this workspace also doesn't have.
//! Rather than add a dependency that covers one of three codecs and call
//! the other two "close enough", [`compress_one`] is a documented
//! `todo!()` and no image codec dependency is added in this slice; the
//! full scan/thread-pool/report/abort machinery around it is real and
//! independently testable via [`get_compressible_images`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Result;

use super::container::Container;

/// Port of `get_compressible_images`: every manifest name whose media
/// type is `image/{png,jpg,jpeg,webp}`.
pub fn get_compressible_images(container: &mut Container) -> Result<HashSet<String>> {
    let type_map = container.manifest_type_map()?;
    let mut images = HashSet::new();
    for ext in ["png", "jpg", "jpeg", "webp"] {
        if let Some(names) = type_map.get(&format!("image/{ext}")) {
            images.extend(names.iter().cloned());
        }
    }
    Ok(images)
}

/// The outcome of compressing one image. Port of the Worker's
/// `results[name] = (True, (before, after))` / `(False, traceback)`
/// shape.
#[derive(Debug, Clone)]
pub struct CompressOutcome {
    pub ok: bool,
    pub before: u64,
    pub after: u64,
    pub error: Option<String>,
}

/// Port of `Worker.compress`'s actual codec dispatch. Genuinely blocked
/// -- see the module docs.
fn compress_one(
    _path: &Path,
    _mime_type: &str,
    _jpeg_quality: Option<u8>,
    _webp_quality: Option<u8>,
) -> Result<(u64, u64)> {
    todo!(
        "placeholder: real image recompression needs codec/optimizer support \
         this crate doesn't have (Qt-backed JPEG/WebP encode + multi-pass PNG \
         optimization in Python) -- see the module docs for why the `image` \
         crate alone isn't a full substitute for this slice"
    )
}

/// Port of `compress_images`. `progress_callback(done, total, name)`
/// returning `false` requests early abort (matching Python's
/// `abort.set()`); the compression work itself happens on a bounded
/// thread pool (Python's `Worker` threads), with `progress_callback`
/// invoked on the calling thread as each result arrives.
pub fn compress_images(
    container: &mut Container,
    names: Option<&HashSet<String>>,
    jpeg_quality: Option<u8>,
    webp_quality: Option<u8>,
    mut report: Option<&mut dyn FnMut(&str)>,
    mut progress_callback: impl FnMut(usize, usize, &str) -> bool,
) -> Result<(bool, HashMap<String, CompressOutcome>)> {
    let mut images = get_compressible_images(container)?;
    if let Some(names) = names {
        images.retain(|n| names.contains(n));
    }
    let mut sorted_images: Vec<String> = images.into_iter().collect();
    sorted_images.sort();

    let mut work: Vec<(String, PathBuf, String)> = Vec::new();
    let mut seen_paths = HashSet::new();
    for name in sorted_images {
        let path = container.get_file_path_for_processing(&name, false)?;
        let mime = container
            .base
            .mime_map
            .get(&name)
            .cloned()
            .unwrap_or_default();
        let path = path.canonicalize().unwrap_or(path);
        let key = path.to_string_lossy().to_lowercase();
        if seen_paths.insert(key) {
            work.push((name, path, mime));
        }
    }
    let num_to_process = work.len();
    progress_callback(0, num_to_process, "");

    let queue = Arc::new(Mutex::new(work.into_iter()));
    let abort = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(String, CompressOutcome)>();
    let num_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(num_to_process.max(1))
        .max(1);

    let results = thread::scope(|scope| {
        for _ in 0..num_workers {
            let queue = Arc::clone(&queue);
            let abort = Arc::clone(&abort);
            let tx = tx.clone();
            scope.spawn(move || {
                while !abort.load(Ordering::SeqCst) {
                    let item = queue.lock().unwrap().next();
                    let Some((name, path, mime)) = item else {
                        break;
                    };
                    let outcome = match compress_one(&path, &mime, jpeg_quality, webp_quality) {
                        Ok((before, after)) => CompressOutcome {
                            ok: true,
                            before,
                            after,
                            error: None,
                        },
                        Err(e) => CompressOutcome {
                            ok: false,
                            before: 0,
                            after: 0,
                            error: Some(e.to_string()),
                        },
                    };
                    if tx.send((name, outcome)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut results = HashMap::new();
        while let Ok((name, outcome)) = rx.recv() {
            let keep_going = progress_callback(results.len() + 1, num_to_process, &name);
            results.insert(name, outcome);
            if !keep_going {
                abort.store(true, Ordering::SeqCst);
            }
        }
        results
    });

    let mut before_total = 0u64;
    let mut after_total = 0u64;
    let mut processed_num = 0u32;
    let mut changed = false;
    for (name, outcome) in &results {
        if outcome.ok {
            if outcome.before != outcome.after {
                changed = true;
                processed_num += 1;
            }
            before_total += outcome.before;
            after_total += outcome.after;
            if let Some(r) = report.as_mut() {
                if outcome.before != outcome.after {
                    let reduction = (outcome.before - outcome.after) as f64 / outcome.before as f64;
                    r(&format!(
                        "{name} compressed from {} to {} bytes [{:.1}% reduction]",
                        outcome.before,
                        outcome.after,
                        reduction * 100.0
                    ));
                } else {
                    r(&format!("{name} could not be further compressed"));
                }
            }
        } else if let Some(r) = report.as_mut() {
            r(&format!("Failed to process {name} with error:"));
            if let Some(e) = &outcome.error {
                r(e);
            }
        }
    }
    if let Some(r) = report.as_mut() {
        if changed {
            r("");
            let reduction = if before_total > 0 {
                (before_total - after_total) as f64 / before_total as f64
            } else {
                0.0
            };
            r(&format!(
                "Total image filesize reduced from {before_total} to {after_total} [{:.1}% reduction, {processed_num} images changed]",
                reduction * 100.0
            ));
        } else {
            r("Images are already fully optimized");
        }
    }
    Ok((changed, results))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(files: &[(&str, &str)]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt) in files {
            fs::write(dir.path().join(name), b"x").unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine></spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn get_compressible_images_filters_by_media_type() {
        let (_dir, mut container) = make_container(&[
            ("a.png", "image/png"),
            ("b.jpg", "image/jpeg"),
            ("c.gif", "image/gif"),
            ("d.webp", "image/webp"),
            ("e.css", "text/css"),
        ]);
        let mut images: Vec<String> = get_compressible_images(&mut container)
            .unwrap()
            .into_iter()
            .collect();
        images.sort();
        // `b.jpg`'s media-type ("image/jpeg") matches the "jpeg" bucket
        // in Python's `'png jpg jpeg webp'.split()` scan, so it's
        // included even though its extension is "jpg", not "jpeg".
        assert_eq!(
            images,
            vec![
                "a.png".to_string(),
                "b.jpg".to_string(),
                "d.webp".to_string()
            ]
        );
    }

    #[test]
    fn get_compressible_images_honors_names_filter_pattern() {
        let (_dir, mut container) =
            make_container(&[("a.png", "image/png"), ("b.jpg", "image/jpeg")]);
        let images = get_compressible_images(&mut container).unwrap();
        let only_a: HashSet<String> = ["a.png".to_string()].into_iter().collect();
        let filtered: HashSet<&String> = images.intersection(&only_a).collect();
        assert_eq!(filtered.len(), 1);
    }
}
