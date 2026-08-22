use base64::Engine;
use calibre_db::backend::Backend;
use calibre_db::notes::connection::NotesConnection;
use calibre_db::notes::exim::{export_note, import_note, ExportResource};
use std::collections::HashSet;
use std::fs;
use tempfile::tempdir;

fn open_notes(dir: &std::path::Path) -> NotesConnection {
    let backend = Backend::new(dir).unwrap();
    let notes = NotesConnection::new(backend, dir);
    notes.initialize().unwrap();
    notes
}

#[test]
fn import_note_stores_a_base64_data_url_image_and_rewrites_src() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());

    let png_bytes = b"\x89PNG fake bytes";
    let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    let html = format!(r#"<p>hello <img src="data:image/png;base64,{b64}"></p>"#);

    let (doc, text, resources) = import_note(&html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    assert_eq!(resources.len(), 1);
    let hash = resources.iter().next().unwrap();
    assert!(doc.contains(&format!("calres://")));
    assert!(doc.contains(hash.split(':').nth(1).unwrap()));
    assert!(text.contains("hello"));

    let stored = notes.get_resource_data(hash).unwrap().unwrap();
    assert_eq!(stored.data, png_bytes);
}

#[test]
fn import_note_rejects_a_non_base64_data_url() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());
    let html = r#"<img src="data:image/png,plaintext-not-base64">"#;

    let (doc, _text, resources) = import_note(html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    assert!(resources.is_empty());
    // Left untouched -- still the original data: URL.
    assert!(doc.contains("data:image/png,plaintext-not-base64"));
}

#[test]
fn import_note_stores_a_local_file_within_basedir() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());
    fs::write(dir.path().join("cover.jpg"), b"jpeg bytes").unwrap();

    let html = r#"<img src="cover.jpg">"#;
    let (doc, _text, resources) = import_note(html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    assert_eq!(resources.len(), 1);
    let hash = resources.iter().next().unwrap();
    assert!(doc.contains("calres://"));
    let stored = notes.get_resource_data(hash).unwrap().unwrap();
    assert_eq!(stored.data, b"jpeg bytes");
    assert_eq!(stored.name, "cover.jpg");
}

#[test]
fn import_note_rejects_a_path_that_escapes_basedir() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), b"should not be read").unwrap();
    let notes = open_notes(dir.path());

    let escape = format!(
        "../{}/secret.txt",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    let html = format!(r#"<img src="{escape}">"#);

    let (_doc, _text, resources) = import_note(&html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    assert!(resources.is_empty());
}

#[test]
fn import_note_leaves_a_remote_url_untouched() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());
    let html = r#"<img src="https://example.com/image.png">"#;

    let (doc, _text, resources) = import_note(html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    assert!(resources.is_empty());
    assert!(doc.contains("https://example.com/image.png"));
}

#[test]
fn export_note_expands_a_resource_placeholder_into_a_data_url() {
    let doc = "<p>see <img src=\"calres://siphash64/abcd1234\"></p>";
    let data = b"raw image bytes".to_vec();
    let expanded = export_note(doc, |hash| {
        assert_eq!(hash, "siphash64:abcd1234");
        Some(ExportResource {
            name: "pic.png".to_string(),
            data: data.clone(),
        })
    });

    assert!(expanded.contains("data:image/png;base64,"));
    assert!(expanded.contains("data-filename=\"pic.png\""));
}

#[test]
fn export_note_leaves_a_non_resource_src_untouched() {
    let doc = r#"<img src="https://example.com/x.png">"#;
    let expanded = export_note(doc, |_| panic!("get_resource should not be called"));
    assert!(expanded.contains("https://example.com/x.png"));
}

#[test]
fn export_note_ignores_an_unresolvable_resource_hash() {
    let doc = r#"<img src="calres://siphash64/missing">"#;
    let expanded = export_note(doc, |_| None);
    // Left as the original placeholder since `get_resource` found nothing.
    assert!(expanded.contains("calres://siphash64/missing"));
}

#[test]
fn round_trip_import_then_export_recovers_the_original_bytes() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());
    let png_bytes = b"round trip bytes";
    let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    let html = format!(r#"<img src="data:image/png;base64,{b64}" data-filename="art.png">"#);

    let (doc, _text, resources) = import_note(&html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });
    assert_eq!(resources.len(), 1);

    let expanded = export_note(&doc, |hash| {
        notes
            .get_resource_data(hash)
            .unwrap()
            .map(|r| ExportResource {
                name: r.name,
                data: r.data,
            })
    });

    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    assert!(expanded.contains(&expected_b64));
}

#[test]
fn import_note_wires_into_notes_connection_set_note() {
    let dir = tempdir().unwrap();
    let notes = open_notes(dir.path());
    let html = r#"<p>plain note text</p>"#;

    let (doc, text, resources) = import_note(html, dir.path(), |data, name| {
        notes.add_resource(data, name).unwrap()
    });

    let joined: HashSet<String> = resources;
    notes.set_note("tags", 1, "fiction", &doc, &joined).unwrap();

    let data = notes.get_note_data("tags", 1).unwrap().unwrap();
    assert_eq!(data.doc, doc);
    assert!(data.searchable_text.contains(&text) || data.searchable_text.contains("fiction"));
}
