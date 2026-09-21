//! calibre-oxide desktop app — Tauri backend.
//!
//! This app is a native shell around the real, already-working
//! `web/` browser UI and `calibre_srv` backend (issue: real desktop
//! app + E2E testing plan) rather than a separate frontend talking to
//! `calibre_db`/`calibre_ebooks` in-process. On startup (or once a
//! library is chosen), this spawns the compiled `calibre_srv` binary
//! pointed at the chosen library with `--static-dir` set to the built
//! `web/dist`, waits for it to be ready, and navigates the window to
//! it. From that point on, the real `web/` UI (library browsing,
//! reading, add/edit) talks to that spawned `calibre_srv` exactly the
//! way it already does when served directly -- no new frontend code,
//! no duplicated UI.
//!
//! `ping` remains as a minimal smoke-test command from the earlier
//! scaffold; the real work now lives in [`server`] (spawn/health-
//! check/shutdown, binary/asset resolution) and [`settings`]
//! (persisted library path).

mod library_import;
mod menu;
mod page_event;
mod server;
mod settings;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, State, Url};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
fn ping() -> String {
    format!("calibre-oxide backend v{} online", env!("CARGO_PKG_VERSION"))
}

/// Holds the spawned `calibre_srv` child process (if any) and the
/// port it's listening on, so it can be killed on app exit and so
/// repeated "open library" calls replace rather than leak a previous
/// server.
#[derive(Default)]
struct ServerState(Mutex<Option<(std::process::Child, u16)>>);

impl ServerState {
    fn kill(&self) {
        if let Some((mut child, _)) = self.0.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawns `calibre_srv` for `library_path`, waits for it to be ready,
/// and navigates the main window to it. Shared by app startup (a
/// previously-persisted library) and the "choose library" flow (a
/// freshly-picked one).
fn open_library(app: &AppHandle, library_path: std::path::PathBuf) -> Result<(), String> {
    let state = app.state::<ServerState>();
    state.kill();

    let bin = server::resolve_calibre_srv_binary(app).map_err(|e| e.to_string())?;
    let static_dir = server::resolve_web_dist(app).map_err(|e| e.to_string())?;
    let port = server::find_free_port().map_err(|e| e.to_string())?;

    let child = server::spawn(&bin, &library_path, &static_dir, port).map_err(|e| e.to_string())?;
    *state.0.lock().unwrap() = Some((child, port));

    if !server::wait_until_ready(port, std::time::Duration::from_secs(10)) {
        return Err(format!("calibre_srv did not become ready on port {port} within 10s"));
    }

    let url = Url::parse(&format!("http://127.0.0.1:{port}/")).map_err(|e| e.to_string())?;
    let window = app.get_webview_window("main").ok_or("no main window")?;
    window.navigate(url).map_err(|e| e.to_string())?;

    let _ = settings::save_library_path(app, &library_path);
    Ok(())
}

/// `GET` (read-only) side of the library-selection flow: what the
/// frontend's initial loading screen calls to decide whether to show
/// a "choose a library" prompt or just wait for the automatic
/// startup navigation.
#[tauri::command]
fn get_persisted_library(app: AppHandle) -> Option<String> {
    settings::load_library_path(&app).map(|p| p.to_string_lossy().into_owned())
}

/// Opens a real native folder-picker dialog; on a real pick, spawns
/// `calibre_srv` for it and navigates the window there.
#[tauri::command]
async fn choose_library(app: AppHandle) -> Result<bool, String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(picked)) = rx.recv().await else {
        return Ok(false);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    open_library(&app, path)?;
    Ok(true)
}

/// Real whole-library import (issue #761): picks a real `.zip` archive
/// (the export side, `GET /library/export/{library_id}` on
/// `calibre_srv`, produces exactly this shape -- a zip of the
/// library's own on-disk layout), then a real destination parent
/// folder, extracts the archive into a new subfolder there named
/// after the archive's own filename, and opens it exactly like a
/// freshly-picked library. Refuses to overwrite an existing directory
/// rather than silently merging into it.
#[tauri::command]
async fn import_library_archive(app: AppHandle) -> Result<bool, String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().add_filter("Library archive", &["zip"]).pick_file(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(archive)) = rx.recv().await else {
        return Ok(false);
    };
    let archive_path = archive.into_path().map_err(|e| e.to_string())?;

    let (tx2, mut rx2) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx2.try_send(result);
    });
    let Some(Some(picked_parent)) = rx2.recv().await else {
        return Ok(false);
    };
    let parent_dir = picked_parent.into_path().map_err(|e| e.to_string())?;

    let stem = archive_path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "imported-library".to_string());
    let dest = parent_dir.join(stem);
    if dest.exists() {
        return Err(format!("{} already exists -- choose a different destination", dest.display()));
    }

    library_import::extract_zip(&archive_path, &dest)?;
    open_library(&app, dest)?;
    Ok(true)
}

/// Real "switch library" quick-switch UI's data source (issue #725):
/// most-recently-opened libraries, newest first. This app stays
/// single-library-per-instance (see `settings.rs`'s own doc) --
/// switching means re-spawning `calibre_srv` against a different
/// path, not serving several libraries at once.
#[tauri::command]
fn list_recent_libraries(app: AppHandle) -> Vec<String> {
    settings::list_recent_libraries(&app).into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// Re-opens a library from the recent-libraries list without going
/// through the folder-picker dialog again.
#[tauri::command]
async fn open_recent_library(app: AppHandle, path: String) -> Result<(), String> {
    open_library(&app, std::path::PathBuf::from(path))
}

/// Issue #721's one real app-level preference: whether to reopen the
/// last library automatically on launch (see `settings.rs`'s own doc
/// on why this defaults to `true`).
#[tauri::command]
fn get_auto_reopen(app: AppHandle) -> bool {
    settings::get_auto_reopen(&app)
}

#[tauri::command]
fn set_auto_reopen(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::set_auto_reopen(&app, enabled).map_err(|e| e.to_string())
}

/// Real extensions `calibre_ebooks::metadata::get_metadata`'s own
/// dispatch table understands (`crates/calibre_ebooks/src/metadata/mod.rs`)
/// -- sourced from that match arm list directly, not invented, so a
/// folder-import scan only picks up files the add-book pipeline can
/// actually read metadata from.
const KNOWN_EBOOK_EXTENSIONS: &[&str] = &[
    "epub", "mobi", "prc", "azw", "azw3", "fb2", "lit", "pdf", "rb", "imp", "lrf", "lrx", "azw4", "chm", "docx", "odt", "snb", "pdb", "updb", "txt", "rtf", "html", "htm", "xhtml", "zip", "cbz", "rar", "cbr",
];

fn simple_job_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("app-add-folder-{now}-{n}")
}

/// `CustomEvent` name announcing a completed drag-and-drop add. Must
/// match the listener in `web/src/components/LibraryView.vue`.
const BOOKS_ADDED_EVENT: &str = "oxide:books-added";

#[derive(serde::Serialize, Default)]
struct AddFolderResult {
    added: u32,
    duplicates: Vec<String>,
    errors: Vec<String>,
}

/// Real folder-import counterpart to `web/`'s own per-file `addBook()`
/// (`web/src/library/api.ts`) -- a plain browser file input can't read
/// an arbitrary folder's worth of files by path, so this drives the
/// same `/cdb/add-book` endpoint directly from the Tauri backend
/// instead, which *can* pick a real folder (`tauri-plugin-dialog`) and
/// read real files from it. Flat (non-recursive) scan for this first
/// slice -- matches upstream `calibredb add`'s own default without
/// `-r`, a real, disclosed narrowing rather than silently always
/// recursing.
#[tauri::command]
async fn choose_folder_and_add_books(app: AppHandle, state: State<'_, ServerState>) -> Result<Option<AddFolderResult>, String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(picked)) = rx.recv().await else {
        return Ok(None);
    };
    let dir = picked.into_path().map_err(|e| e.to_string())?;

    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;

    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).map(|e| KNOWN_EBOOK_EXTENSIONS.contains(&e.to_lowercase().as_str())).unwrap_or(false))
        .collect();
    files.sort();

    Ok(Some(add_files_via_server(port, &files).await))
}

/// Uploads each file to the running `calibre_srv`'s `/cdb/add-book`,
/// exactly as `web/`'s own per-file `addBook()` does.
///
/// Shared by the folder picker above and the drag-and-drop handler
/// (issue #818) so the two paths cannot disagree about duplicate
/// handling or error reporting.
async fn add_files_via_server(port: u16, files: &[std::path::PathBuf]) -> AddFolderResult {
    let client = reqwest::Client::new();
    let mut result = AddFolderResult::default();
    for path in files {
        let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                result.errors.push(format!("{filename}: {e}"));
                continue;
            }
        };
        let url = format!("http://127.0.0.1:{port}/cdb/add-book/{}/n/{}/-", simple_job_id(), urlencoding::encode(&filename));
        match client.post(&url).body(bytes).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body.get("book_id").is_some() => result.added += 1,
                Ok(_) => result.duplicates.push(filename),
                Err(e) => result.errors.push(format!("{filename}: {e}")),
            },
            Ok(resp) => result.errors.push(format!("{filename}: HTTP {}", resp.status())),
            Err(e) => result.errors.push(format!("{filename}: {e}")),
        }
    }
    result
}

/// Keeps only the files this app knows how to read metadata from.
/// Applied to dropped paths, which -- unlike a folder scan -- can be
/// anything the user happened to drag.
fn keep_known_ebooks(paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = paths
        .iter()
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).map(|e| KNOWN_EBOOK_EXTENSIONS.contains(&e.to_lowercase().as_str())).unwrap_or(false))
        .cloned()
        .collect();
    files.sort();
    files
}

/// Rebuilds the native menu bar from the page's own action registry.
///
/// See `menu.rs` for why the page is the authority here rather than a
/// hardcoded Rust-side list.
#[tauri::command]
fn set_menu_actions(app: AppHandle, actions: Vec<menu::MenuActionSpec>) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("no main window")?;
    menu::install(&window, &actions).map_err(|e| e.to_string())
}

/// Opens one of a book's formats in the OS default application.
///
/// This is how PDFs are read today: the in-app reader is EPUB/KEPUB
/// only (`is_viewable_format` in `calibre_srv`), so without this a
/// PDF-first library has no way to open its own books.
///
/// The bytes come from the running `calibre_srv` rather than from the
/// library folder directly, because `/ajax/book` deliberately strips
/// the internal `fmt_<ext>` absolute paths before they reach the page
/// -- correct for an API that can be served over a network, and not
/// worth undoing for the desktop case. The trade-off is that this
/// opens a copy under the temp directory, so edits made in an external
/// application do not flow back into the library.
#[tauri::command]
async fn open_book_format(state: State<'_, ServerState>, app: AppHandle, book_id: i32, fmt: String) -> Result<(), String> {
    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;

    let ext = fmt.to_lowercase();
    if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("{fmt:?} is not a usable format name"));
    }

    let url = format!("http://127.0.0.1:{port}/get/{ext}/{book_id}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("could not fetch the {} for book {book_id}: HTTP {}", ext.to_uppercase(), resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    let dir = std::env::temp_dir().join("calibre-oxide-open");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // `book_id` is an integer and `ext` is alphanumeric-checked above,
    // so this name cannot escape `dir`.
    let path = dir.join(format!("{book_id}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(ServerState::default())
        .invoke_handler(tauri::generate_handler![ping, get_persisted_library, choose_library, choose_folder_and_add_books, list_recent_libraries, open_recent_library, get_auto_reopen, set_auto_reopen, import_library_archive, set_menu_actions, open_book_format])
        .on_menu_event(|app, event| menu::forward(app, &event))
        // Dropping files onto the window adds them, the same way the
        // folder picker does. This has to be handled natively: Tauri's
        // own drag-drop handling suppresses the webview's HTML5 drop
        // events, so a listener on the page would never fire.
        .on_window_event(|window, event| {
            let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event else {
                return;
            };
            let files = keep_known_ebooks(paths);
            if files.is_empty() {
                return;
            }
            let Some(webview) = window.get_webview_window("main") else {
                return;
            };
            let Some(port) = webview.state::<ServerState>().0.lock().unwrap().as_ref().map(|(_, p)| *p) else {
                page_event::dispatch(&webview, BOOKS_ADDED_EVENT, &serde_json::json!({ "error": "no library is currently open" }));
                return;
            };
            tauri::async_runtime::spawn(async move {
                let result = add_files_via_server(port, &files).await;
                // The web UI does not listen on calibre_srv's
                // websocket, so the grid will not refresh on its own
                // -- tell the page directly.
                page_event::dispatch(&webview, BOOKS_ADDED_EVENT, &result);
            });
        })
        .setup(|app| {
            // Auto-open the last library, if any, without waiting for
            // the frontend to ask -- real startup UX, not just a
            // technically-correct command surface. Gated on the
            // auto_reopen preference (issue #721): a user who's
            // disabled it wants to land on the loading screen's
            // "choose a library" prompt instead every time.
            if settings::get_auto_reopen(app.handle()) {
                if let Some(path) = settings::load_library_path(app.handle()) {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        if let Err(e) = open_library(&handle, path) {
                            eprintln!("failed to reopen the last library on startup: {e}");
                        }
                    });
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app_handle.state::<ServerState>().kill();
            }
        });
}
