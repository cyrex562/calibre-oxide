//! calibre-oxide desktop app — Tauri backend.
//!
//! Commands invoked from the Vue front-end land here. The pattern is:
//!
//! - Front-end calls `invoke("cmd_name", args)`
//! - We deserialize args, dispatch to the appropriate crate
//!   (calibre_db, calibre_ebooks, ...), serialize the result.
//! - Every operation that touches library state MUST go through a
//!   `LibraryHandle` per docs/FAULT_TOLERANCE.md — no raw filesystem
//!   calls from Tauri commands.

#[tauri::command]
fn ping() -> String {
    format!("calibre-oxide backend v{} online", env!("CARGO_PKG_VERSION"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
