// Declares the app's commands to Tauri's access-control layer.
//
// # Why this is needed at all
//
// The window does not display a bundled frontend. It navigates to
// `http://127.0.0.1:<port>/`, served by the `calibre_srv` the app
// spawns — so as far as Tauri is concerned the page is *remote*
// content, not local. `Webview::on_message` (tauri/src/webview/mod.rs)
// rejects every command from a non-local origin unless an explicit
// capability permits it:
//
//     if (plugin_command.is_some() || has_app_acl_manifest || !is_local)
//         && invoke.acl.is_none() { reject("not allowed by ACL") }
//
// With no manifest and no capabilities, the splash worked — it is
// local, uses no plugin commands over IPC, and there was no app
// manifest, so all three conditions were false. Everything the served
// UI invoked failed with "not allowed by ACL".
//
// # The trap in fixing it
//
// Declaring the manifest makes `has_app_acl_manifest` *true*, which
// then requires a capability for local content too. So the capability
// files must cover both origins, or fixing the served UI would break
// the splash that already worked.

const COMMANDS: &[&str] = &[
    "ping",
    // Library lifecycle
    "get_persisted_library",
    "choose_library",
    "create_library",
    "list_recent_libraries",
    "open_recent_library",
    "import_library_archive",
    // Adding books
    "choose_folder_and_add_books",
    "get_auto_add_folder",
    "choose_auto_add_folder",
    // Books on disk
    "open_book_format",
    "unpack_book",
    "repack_book",
    "open_external_url",
    // App shell
    "set_menu_actions",
    "get_auto_reopen",
    "set_auto_reopen",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            // Generates an `allow-<command>` permission for each, with
            // underscores slugified to hyphens — so `set_menu_actions`
            // becomes `allow-set-menu-actions`. The capability files in
            // `capabilities/` reference those names.
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
