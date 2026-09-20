//! End-to-end check that the plugin registry (#795) actually drives
//! real work: a real PML file, registered builtins, and the registry-
//! backed import path producing a real PMLZ archive.

use calibre_customize::registry::PluginRegistry;
use calibre_customize::ui::run_plugins_on_import_from_registry;

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn a_registered_builtin_really_packages_a_real_pml_on_import() {
    let tmp = tempfile::tempdir().unwrap();
    let pml = tmp.path().join("story.pml");
    std::fs::write(&pml, "\\x chapter one\nsome pml body text\n").unwrap();
    let img_dir = tmp.path().join("story_img");
    std::fs::create_dir(&img_dir).unwrap();
    std::fs::write(img_dir.join("cover.png"), PNG_1X1).unwrap();
    std::fs::write(img_dir.join("b.png"), PNG_1X1).unwrap();
    std::fs::write(img_dir.join("skip.jpg"), b"nope").unwrap();

    let mut registry = PluginRegistry::new();
    calibre_customize::builtins::register_builtins(&mut registry);
    calibre_ebooks::plugins::register_builtins(&mut registry).unwrap();

    let out = run_plugins_on_import_from_registry(&pml, &registry);

    assert_eq!(out, tmp.path().join("story.pmlz"), "import should have produced a real .pmlz");
    assert!(out.exists());

    let archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut names: Vec<String> = archive.file_names().map(str::to_string).collect();
    names.sort();

    // Cross-validated against real upstream PML2PMLZ.run executed on the
    // identical input tree, which produces exactly this entry set.
    assert_eq!(names, ["images/b.png", "images/cover.png", "story.pml"]);
}

#[test]
fn disabling_the_builtin_really_stops_it_packaging_on_import() {
    let tmp = tempfile::tempdir().unwrap();
    let pml = tmp.path().join("story.pml");
    std::fs::write(&pml, "content").unwrap();

    let mut registry = PluginRegistry::new();
    calibre_ebooks::plugins::register_builtins(&mut registry).unwrap();
    registry.set_enabled("PML to PMLZ", false).unwrap();

    let out = run_plugins_on_import_from_registry(&pml, &registry);

    assert_eq!(out, pml, "a disabled plugin must leave the import path untouched");
    assert!(!tmp.path().join("story.pmlz").exists(), "and must not have produced an archive");
}
