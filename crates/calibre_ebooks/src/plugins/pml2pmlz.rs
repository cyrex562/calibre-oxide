//! Port of upstream's `PML2PMLZ` builtin `FileTypePlugin`
//! (`calibre/customize/builtins.py`), the first real registered plugin
//! in this port (issue #795).
//!
//! Upstream's own description: *"Create a PMLZ archive containing the
//! PML file and all images in the folder pmlname_img or images. This
//! plugin is run every time you add a PML file to the library."*
//!
//! # Faithfulness notes
//!
//! - Image folder resolution matches upstream exactly: prefer
//!   `<stem>_img/` next to the PML file, else `images/` in the same
//!   directory, else no images at all.
//! - Only `*.png` is collected, matching upstream's single
//!   `glob(os.path.join(img_dir, '*.png'))`. That looks narrow for an
//!   image format that also sees `.jpg` in the wild, but it is what
//!   upstream does, and silently widening it here would make this port
//!   produce different archives than calibre for the same input.
//! - Images are stored under `images/` inside the archive regardless of
//!   which source folder they came from, again matching upstream.
//!
//! # Deliberate difference: where the output goes
//!
//! Upstream writes to `self.temporary_file('_plugin_pml2pmlz.pmlz')` --
//! a tempfile owned by the plugin base class and cleaned up by
//! calibre's own tempfile machinery. This port has no equivalent
//! plugin-owned temp-file service, so the archive is written next to
//! the source file as `<stem>.pmlz`. Disclosed rather than silently
//! different; if a temp-file service is added later this should move to
//! it.

use std::io::Write;
use std::path::{Path, PathBuf};

use calibre_customize::{FileTypePlugin, Plugin, PluginInstallationType};

pub struct Pml2Pmlz;

impl Plugin for Pml2Pmlz {
    fn name(&self) -> &str {
        "PML to PMLZ"
    }

    fn author(&self) -> &str {
        "John Schember"
    }

    fn description(&self) -> &str {
        "Create a PMLZ archive containing the PML file and all images in the folder pmlname_img or images. This plugin is run every time you add a PML file to the library."
    }

    fn installation_type(&self) -> Option<PluginInstallationType> {
        Some(PluginInstallationType::Builtin)
    }

    fn type_name(&self) -> &str {
        "File type"
    }
}

impl FileTypePlugin for Pml2Pmlz {
    fn file_types(&self) -> Vec<String> {
        vec!["pml".to_string()]
    }

    fn on_import(&self) -> bool {
        true
    }

    /// Returns the path to the new `.pmlz`, or -- matching the
    /// crate-wide `FileTypePlugin::run` contract and upstream's own
    /// tolerance of a failing plugin -- the original path unchanged if
    /// the archive could not be written.
    fn run(&self, path_to_ebook: &Path) -> PathBuf {
        match package_pmlz(path_to_ebook) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("PML to PMLZ: failed to package {}: {e}", path_to_ebook.display());
                path_to_ebook.to_path_buf()
            }
        }
    }
}

/// Port of upstream `PML2PMLZ.run`'s body. Split out from the trait
/// method so it can return a real error for testing instead of being
/// forced into the trait's infallible `PathBuf` return.
pub fn package_pmlz(pml_path: &Path) -> anyhow::Result<PathBuf> {
    let parent = pml_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = pml_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let out_path = parent.join(format!("{stem}.pmlz"));

    let file = std::fs::File::create(&out_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // The PML file itself, stored under its own basename.
    let pml_name = pml_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    zip.start_file(&pml_name, options)?;
    zip.write_all(&std::fs::read(pml_path)?)?;

    if let Some(img_dir) = image_dir_for(pml_path) {
        // Sorted for a deterministic archive; upstream's `glob` order is
        // filesystem-dependent, which would make the output unstable.
        let mut pngs: Vec<PathBuf> = std::fs::read_dir(&img_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png")))
            .collect();
        pngs.sort();

        for image in pngs {
            let name = image.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            zip.start_file(format!("images/{name}"), options)?;
            zip.write_all(&std::fs::read(&image)?)?;
        }
    }

    zip.finish()?;
    Ok(out_path)
}

/// Port of upstream's image-folder resolution: `<stem>_img` wins,
/// otherwise a sibling `images` folder, otherwise none.
fn image_dir_for(pml_path: &Path) -> Option<PathBuf> {
    let parent = pml_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = pml_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

    let pml_img = parent.join(format!("{stem}_img"));
    if pml_img.is_dir() {
        return Some(pml_img);
    }
    let images = parent.join("images");
    if images.is_dir() {
        return Some(images);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest possible real PNG (1x1, transparent), so the test
    /// archives contain genuine image bytes rather than placeholder
    /// text pretending to be an image.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
        0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn entries(path: &Path) -> Vec<String> {
        let archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = archive.file_names().map(str::to_string).collect();
        names.sort();
        names
    }

    #[test]
    fn a_lone_pml_file_becomes_a_pmlz_containing_just_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"\\x real pml content").unwrap();

        let out = package_pmlz(&pml).unwrap();

        assert_eq!(out, tmp.path().join("story.pmlz"));
        assert_eq!(entries(&out), ["story.pml"]);
    }

    #[test]
    fn the_pml_contents_really_survive_into_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"\\x chapter one").unwrap();

        let out = package_pmlz(&pml).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("story.pml").unwrap(), &mut contents).unwrap();
        assert_eq!(contents, "\\x chapter one");
    }

    #[test]
    fn images_from_the_stem_img_folder_are_included_under_images() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"content").unwrap();
        let img_dir = tmp.path().join("story_img");
        std::fs::create_dir(&img_dir).unwrap();
        std::fs::write(img_dir.join("cover.png"), PNG_1X1).unwrap();

        let out = package_pmlz(&pml).unwrap();
        assert_eq!(entries(&out), ["images/cover.png", "story.pml"]);
    }

    #[test]
    fn a_sibling_images_folder_is_used_when_there_is_no_stem_img_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"content").unwrap();
        let img_dir = tmp.path().join("images");
        std::fs::create_dir(&img_dir).unwrap();
        std::fs::write(img_dir.join("plate.png"), PNG_1X1).unwrap();

        let out = package_pmlz(&pml).unwrap();
        assert_eq!(entries(&out), ["images/plate.png", "story.pml"]);
    }

    #[test]
    fn the_stem_img_folder_wins_over_a_sibling_images_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"content").unwrap();

        let preferred = tmp.path().join("story_img");
        std::fs::create_dir(&preferred).unwrap();
        std::fs::write(preferred.join("chosen.png"), PNG_1X1).unwrap();

        let ignored = tmp.path().join("images");
        std::fs::create_dir(&ignored).unwrap();
        std::fs::write(ignored.join("ignored.png"), PNG_1X1).unwrap();

        let out = package_pmlz(&pml).unwrap();
        assert_eq!(entries(&out), ["images/chosen.png", "story.pml"], "upstream prefers <stem>_img and ignores the images/ folder entirely when it exists");
    }

    #[test]
    fn non_png_files_in_the_image_folder_are_skipped_like_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"content").unwrap();
        let img_dir = tmp.path().join("story_img");
        std::fs::create_dir(&img_dir).unwrap();
        std::fs::write(img_dir.join("keep.png"), PNG_1X1).unwrap();
        std::fs::write(img_dir.join("skip.jpg"), b"not collected by upstream's *.png glob").unwrap();
        std::fs::write(img_dir.join("notes.txt"), b"definitely not an image").unwrap();

        let out = package_pmlz(&pml).unwrap();
        assert_eq!(entries(&out), ["images/keep.png", "story.pml"]);
    }

    #[test]
    fn the_plugin_trait_entry_point_produces_the_same_real_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let pml = tmp.path().join("story.pml");
        std::fs::write(&pml, b"content").unwrap();

        let out = Pml2Pmlz.run(&pml);

        assert_eq!(out, tmp.path().join("story.pmlz"));
        assert!(out.exists());
        assert_eq!(entries(&out), ["story.pml"]);
    }

    #[test]
    fn a_failure_leaves_the_original_path_untouched_rather_than_panicking() {
        // A path that cannot be read: `run` must degrade to returning
        // the input unchanged, the way upstream tolerates a plugin that
        // raises.
        let missing = Path::new("/nonexistent-dir-for-test/story.pml");
        assert_eq!(Pml2Pmlz.run(missing), missing);
    }
}
