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

    // The window says which library is open. Two windows on two
    // libraries are otherwise indistinguishable in a task switcher,
    // where the title is all you get.
    let name = library_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| library_path.to_string_lossy().into_owned());
    let _ = window.set_title(&format!("calibre-oxide — {name}"));

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

/// Creates a new library and opens it.
///
/// Pointing `calibre_srv` at an empty directory already produces a
/// working library -- it creates `metadata.db` and its sidecars on
/// first run. What was missing was any way to *say* that: the only
/// library affordance was "Switch library -> Browse for another",
/// which reads as "find an existing one" and gives no hint that
/// choosing an empty folder makes a new one.
///
/// Asks for a parent folder and a name rather than a single folder
/// pick, because "create" and "open" want different dialogs: picking a
/// folder to create *inside* is a different question from picking the
/// library itself, and conflating them is how a user ends up with a
/// library at the root of their documents folder.
#[tauri::command]
async fn create_library(app: AppHandle, name: String) -> Result<Option<String>, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("a library needs a name".to_string());
    }
    // Rejected rather than sanitized: silently renaming what someone
    // typed produces a folder they then cannot find.
    if trimmed.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || trimmed.starts_with('.') {
        return Err("a library name cannot contain / \\ : * ? \" < > | or start with a dot".to_string());
    }

    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(parent)) = rx.recv().await else {
        return Ok(None);
    };
    let parent = parent.into_path().map_err(|e| e.to_string())?;
    let target = parent.join(&trimmed);

    // Refuse rather than merge. An existing directory may be somebody
    // else's data, and opening it as a library would adopt whatever is
    // inside.
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    std::fs::create_dir(&target).map_err(|e| format!("could not create {}: {e}", target.display()))?;

    open_library(&app, target.clone())?;
    Ok(Some(target.to_string_lossy().into_owned()))
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

    let ext = checked_ext(&fmt)?;

    let bytes = fetch_book_format(port, book_id, &ext).await?;

    let dir = std::env::temp_dir().join("calibre-oxide-open");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // `book_id` is an integer and `ext` is alphanumeric-checked above,
    // so this name cannot escape `dir`.
    let path = dir.join(format!("{book_id}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Fetches one of a book's formats from the running `calibre_srv`.
///
/// Shared by [`open_book_format`] and [`unpack_book`]. Goes through
/// the server rather than the library folder because `/ajax/book`
/// deliberately strips the internal `fmt_<ext>` paths before they
/// reach any client -- correct for an API that can be served over a
/// network, and not worth undoing for the desktop case.
async fn fetch_book_format(port: u16, book_id: i32, ext: &str) -> Result<Vec<u8>, String> {
    let url = format!("http://127.0.0.1:{port}/get/{ext}/{book_id}");
    let resp = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("could not fetch the {} for book {book_id}: HTTP {}", ext.to_uppercase(), resp.status()));
    }
    Ok(resp.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

/// Rejects anything that could escape a directory when used as a file
/// extension. Both callers embed the result in a filesystem path.
fn checked_ext(fmt: &str) -> Result<String, String> {
    let ext = fmt.to_lowercase();
    if ext.is_empty() || ext.len() > 10 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("{fmt:?} is not a usable format name"));
    }
    Ok(ext)
}

async fn pick_folder(app: &AppHandle) -> Option<std::path::PathBuf> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(picked)) = rx.recv().await else {
        return None;
    };
    picked.into_path().ok()
}

#[derive(serde::Serialize)]
struct UnpackResult {
    path: String,
}

/// Unpacks a book into a folder the user picks, for editing with
/// whatever tools they prefer (issue #816 item 1.12).
///
/// `calibre_ebooks::tweak::explode` leaves a hidden marker file
/// recording the source format; [`repack_book`] refuses to rebuild
/// from a folder that has no marker, or whose marker disagrees with
/// the target format. That check is upstream's own, and it is what
/// stops a folder exploded from an EPUB being rebuilt as a MOBI.
///
/// Unpacks into a *subfolder* named after the book rather than
/// directly into the chosen folder: exploding straight into, say, a
/// Documents folder would scatter a book's innards across it.
#[tauri::command]
async fn unpack_book(state: State<'_, ServerState>, app: AppHandle, book_id: i32, fmt: String) -> Result<Option<UnpackResult>, String> {
    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;
    let ext = checked_ext(&fmt)?;

    let Some(parent) = pick_folder(&app).await else {
        return Ok(None);
    };

    let bytes = fetch_book_format(port, book_id, &ext).await?;

    let tmp = std::env::temp_dir().join(format!("calibre-oxide-unpack-{book_id}.{ext}"));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;

    let dest = parent.join(format!("book-{book_id}-{ext}"));
    if dest.exists() {
        return Err(format!("{} already exists -- choose a different folder", dest.display()));
    }

    // The `question` callback answers the joint MOBI6+KF8 prompt.
    // Answering yes is right here: the user explicitly asked to
    // unpack this book, and declining would silently do nothing.
    calibre_ebooks::tweak::explode(&tmp, &dest, |_| true).map_err(|e| e.to_string())?.ok_or_else(|| "that book could not be unpacked".to_string())?;
    let _ = std::fs::remove_file(&tmp);

    let _ = app.opener().reveal_item_in_dir(&dest);
    Ok(Some(UnpackResult { path: dest.to_string_lossy().into_owned() }))
}

/// Rebuilds a book from a folder produced by [`unpack_book`] and adds
/// it back as that format.
#[tauri::command]
async fn repack_book(state: State<'_, ServerState>, app: AppHandle, book_id: i32, fmt: String) -> Result<bool, String> {
    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;
    let ext = checked_ext(&fmt)?;

    let Some(dir) = pick_folder(&app).await else {
        return Ok(false);
    };

    let rebuilt = std::env::temp_dir().join(format!("calibre-oxide-repack-{book_id}.{ext}"));
    let _ = std::fs::remove_file(&rebuilt);
    // `implode` verifies its own marker file, so a folder that was
    // not produced by unpacking -- or was unpacked from a different
    // format -- is refused here rather than producing a broken book.
    calibre_ebooks::tweak::implode(&dir, &rebuilt).map_err(|e| e.to_string())?;

    let bytes = std::fs::read(&rebuilt).map_err(|e| e.to_string())?;
    let data_url = format!("data:application/octet-stream;base64,{}", base64_encode(&bytes));
    let _ = std::fs::remove_file(&rebuilt);

    let url = format!("http://127.0.0.1:{port}/cdb/set-fields/{book_id}");
    let body = serde_json::json!({ "changes": { "added_formats": [{ "ext": ext, "data_url": data_url }] } });
    let resp = reqwest::Client::new().post(&url).json(&body).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("could not store the rebuilt book: HTTP {}", resp.status()));
    }
    Ok(true)
}

/// Minimal base64 for the `data_url` the set-fields route expects.
/// Written out rather than pulling in a crate for one call site.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Opens a URL in the user's browser (#816's dictionary-lookup item).
///
/// Restricted to http/https: this takes a URL built by the page, and
/// handing an arbitrary scheme to the OS opener is how a `file://` or
/// a custom-protocol URL turns a lookup into something else entirely.
#[tauri::command]
async fn open_external_url(app: AppHandle, url: String) -> Result<(), String> {
    let parsed = Url::parse(&url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("refusing to open a {:?} URL", parsed.scheme()));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ===================================================================
// Auto-add folder (#816 item 4.4)
// ===================================================================
//
// A folder watched for new books, so dropping a file into it adds it
// to the library without opening the app.
//
// Polled rather than using filesystem notifications: a notification
// fires the moment a file *appears*, which for anything arriving over
// a network share or a browser download is while it is still being
// written -- adding a half-copied book is worse than adding it a few
// seconds later. Polling with a stability check avoids that without a
// watcher dependency.

/// How often the folder is scanned.
const AUTO_ADD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// A file must be this old, and unchanged, before it is added --
/// long enough that a copy still in progress is not mistaken for a
/// finished one.
const AUTO_ADD_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// Files in `dir` that look finished and are worth adding.
fn auto_add_candidates(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let now = std::time::SystemTime::now();

    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            if !p.is_file() {
                return false;
            }
            let known = p.extension().and_then(|e| e.to_str()).map(|e| KNOWN_EBOOK_EXTENSIONS.contains(&e.to_lowercase().as_str())).unwrap_or(false);
            if !known {
                return false;
            }
            // Still settling: almost certainly still being written.
            std::fs::metadata(p).and_then(|m| m.modified()).map(|m| now.duration_since(m).map(|age| age >= AUTO_ADD_SETTLE).unwrap_or(false)).unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

#[tauri::command]
fn get_auto_add_folder(app: AppHandle) -> Option<String> {
    settings::get_auto_add_folder(&app).map(|p| p.to_string_lossy().into_owned())
}

/// Picks a folder to watch, or clears the existing one.
#[tauri::command]
async fn choose_auto_add_folder(app: AppHandle, clear: bool) -> Result<Option<String>, String> {
    if clear {
        settings::set_auto_add_folder(&app, None).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    let Some(folder) = pick_folder(&app).await else {
        return Ok(settings::get_auto_add_folder(&app).map(|p| p.to_string_lossy().into_owned()));
    };
    settings::set_auto_add_folder(&app, Some(folder.clone())).map_err(|e| e.to_string())?;
    Ok(Some(folder.to_string_lossy().into_owned()))
}

/// Starts the background watcher.
///
/// A successfully added file is **removed** from the watched folder.
/// That is the whole point of a drop folder -- leaving it would mean
/// re-adding the same book on every scan, and de-duplicating by name
/// would break the moment someone renamed a file.
fn spawn_auto_add_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tauri::async_runtime::spawn_blocking(|| std::thread::sleep(AUTO_ADD_INTERVAL)).await.ok();

            let Some(folder) = settings::get_auto_add_folder(&app) else { continue };
            let Some(port) = app.state::<ServerState>().0.lock().unwrap().as_ref().map(|(_, p)| *p) else { continue };

            let files = auto_add_candidates(&folder);
            if files.is_empty() {
                continue;
            }

            let result = add_files_via_server(port, &files).await;
            for (path, ()) in files.iter().zip(std::iter::repeat(())) {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                // Only remove what really landed: a file that failed
                // or was a duplicate stays put, so nothing is lost
                // silently.
                if !result.errors.iter().any(|e| e.starts_with(&name)) && !result.duplicates.contains(&name) {
                    let _ = std::fs::remove_file(path);
                }
            }

            if let Some(window) = app.get_webview_window("main") {
                page_event::dispatch(&window, BOOKS_ADDED_EVENT, &result);
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(ServerState::default())
        .invoke_handler(tauri::generate_handler![ping, get_persisted_library, choose_library, create_library, choose_folder_and_add_books, list_recent_libraries, open_recent_library, get_auto_reopen, set_auto_reopen, import_library_archive, set_menu_actions, open_book_format, unpack_book, repack_book, open_external_url, get_auto_add_folder, choose_auto_add_folder])
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
            spawn_auto_add_watcher(app.handle().clone());

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

#[cfg(test)]
mod unpack_tests {
    use super::*;

    /// Cross-checked against known RFC 4648 vectors, because a
    /// hand-written encoder is exactly the kind of thing that is
    /// subtly wrong only on the padded tail.
    #[test]
    fn base64_matches_the_rfc_test_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_bytes_outside_ascii() {
        // Real book bytes are binary; an encoder that only worked on
        // text would corrupt every EPUB it touched.
        assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
        assert_eq!(base64_encode(&[0x00, 0x00, 0x00]), "AAAA");
    }

    #[test]
    fn checked_ext_rejects_path_traversal() {
        // Both callers embed this in a filesystem path.
        assert!(checked_ext("../../etc").is_err());
        assert!(checked_ext("ep/ub").is_err());
        assert!(checked_ext("").is_err());
        assert!(checked_ext("averyverylongextension").is_err());
        assert_eq!(checked_ext("EPUB").unwrap(), "epub");
    }

    /// The round-trip the unpack/repack pair depends on, against a
    /// real EPUB rather than a stub.
    #[test]
    fn explode_then_implode_round_trips_a_real_epub() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("book.epub");
        std::fs::write(&src, real_epub_bytes()).unwrap();

        let exploded = dir.path().join("exploded");
        calibre_ebooks::tweak::explode(&src, &exploded, |_| true).unwrap().expect("explode should succeed");
        assert!(exploded.join("content.opf").exists(), "the OPF should be on disk for external editing");

        // Edit it the way a user would, with their own tools.
        let chapter = exploded.join("chapter1.xhtml");
        let edited = std::fs::read_to_string(&chapter).unwrap().replace("Hello", "Goodbye");
        std::fs::write(&chapter, &edited).unwrap();

        let rebuilt = dir.path().join("rebuilt.epub");
        calibre_ebooks::tweak::implode(&exploded, &rebuilt).unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&rebuilt).unwrap()).unwrap();
        let mut text = String::new();
        {
            use std::io::Read;
            zip.by_name("chapter1.xhtml").unwrap().read_to_string(&mut text).unwrap();
        }
        assert!(text.contains("Goodbye"), "the external edit must survive the rebuild: {text}");
    }

    /// `implode`'s marker check is what stops a folder exploded from
    /// one format being rebuilt as another, or a folder that was
    /// never exploded at all producing a broken book.
    #[test]
    fn implode_refuses_a_folder_that_was_never_exploded() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("just-a-folder");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("something.txt"), b"x").unwrap();

        let out = dir.path().join("out.epub");
        assert!(calibre_ebooks::tweak::implode(&plain, &out).is_err());
    }

    fn real_epub_bytes() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mimetype"), "application/epub+zip").unwrap();
        std::fs::create_dir_all(dir.path().join("META-INF")).unwrap();
        std::fs::write(
            dir.path().join("META-INF/container.xml"),
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("chapter1.xhtml"), r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello, world!</p></body></html>"#).unwrap();
        std::fs::write(
            dir.path().join("content.opf"),
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="bookid">id1</dc:identifier><dc:title>Test</dc:title><dc:language>en</dc:language></metadata>
<manifest><item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/></spine>
</package>"#,
        )
        .unwrap();

        let epub_path = dir.path().join("out.epub");
        let file = std::fs::File::create(&epub_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for name in ["mimetype", "META-INF/container.xml", "content.opf", "chapter1.xhtml"] {
            zip.start_file(name, opts).unwrap();
            use std::io::Write;
            zip.write_all(&std::fs::read(dir.path().join(name)).unwrap()).unwrap();
        }
        zip.finish().unwrap();
        std::fs::read(&epub_path).unwrap()
    }
}

#[cfg(test)]
mod auto_add_tests {
    use super::*;

    fn write_with_age(dir: &std::path::Path, name: &str, seconds_old: u64) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        // Backdate so the settle check sees a finished file.
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds_old);
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(when).unwrap();
        path
    }

    #[test]
    fn picks_up_a_settled_ebook() {
        let dir = tempfile::tempdir().unwrap();
        write_with_age(dir.path(), "book.epub", 60);

        let found = auto_add_candidates(dir.path());

        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("book.epub"));
    }

    /// A file that appeared a moment ago is very likely still being
    /// written -- over a network share or by a browser download --
    /// and adding a half-copied book is worse than adding it a few
    /// seconds later.
    #[test]
    fn ignores_a_file_that_is_still_settling() {
        let dir = tempfile::tempdir().unwrap();
        write_with_age(dir.path(), "arriving.epub", 0);

        assert!(auto_add_candidates(dir.path()).is_empty());
    }

    #[test]
    fn ignores_files_that_are_not_books() {
        let dir = tempfile::tempdir().unwrap();
        write_with_age(dir.path(), "notes.md", 60);
        write_with_age(dir.path(), "archive.tar", 60);
        write_with_age(dir.path(), "real.epub", 60);

        let found = auto_add_candidates(dir.path());

        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("real.epub"));
    }

    #[test]
    fn ignores_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir.epub")).unwrap();

        assert!(auto_add_candidates(dir.path()).is_empty(), "a directory named like a book is not a book");
    }

    #[test]
    fn an_empty_or_missing_folder_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(auto_add_candidates(dir.path()).is_empty());
        assert!(auto_add_candidates(&dir.path().join("does-not-exist")).is_empty());
    }

    /// Stable order, so a batch is added in a predictable sequence
    /// rather than whatever the filesystem happened to return.
    #[test]
    fn results_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c.epub", "a.epub", "b.epub"] {
            write_with_age(dir.path(), name, 60);
        }

        let found: Vec<String> = auto_add_candidates(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();

        assert_eq!(found, vec!["a.epub", "b.epub", "c.epub"]);
    }
}

/// Guards the access-control wiring.
///
/// The window navigates to the `calibre_srv` it spawns, so its page is
/// *remote* content and every command it invokes needs an explicit
/// capability. Getting that wrong does not fail to compile and does not
/// fail any other test -- it fails at runtime, in the user's hands,
/// with "Command X not allowed by ACL". That is how it reached a user
/// the first time.
#[cfg(test)]
mod acl_tests {
    use std::collections::BTreeSet;

    /// The commands `run()` actually registers, read from this file.
    ///
    /// `generate_handler!` does not expose its list, so it is parsed
    /// back out of the source. Coarse, but it is the only way to
    /// compare what is *registered* against what is *permitted*, which
    /// is the pair that drifts.
    fn registered_commands() -> BTreeSet<String> {
        let src = include_str!("lib.rs");
        let start = src.find("generate_handler![").expect("generate_handler! call");
        let body = &src[start + "generate_handler![".len()..];
        let end = body.find(']').expect("closing bracket");
        body[..end]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn permitted_commands(capability: &str) -> BTreeSet<String> {
        let json: serde_json::Value = serde_json::from_str(capability).expect("capability json");
        json["permissions"]
            .as_array()
            .expect("permissions array")
            .iter()
            .filter_map(|p| p.as_str())
            .filter_map(|p| p.strip_prefix("allow-"))
            // The generator slugifies underscores to hyphens; reverse
            // it to compare against the command names.
            .map(|p| p.replace('-', "_"))
            .collect()
    }

    const SERVED_UI: &str = include_str!("../capabilities/served-ui.json");
    const APP_LOCAL: &str = include_str!("../capabilities/app-local.json");

    #[test]
    fn every_registered_command_is_reachable_from_the_served_ui() {
        let registered = registered_commands();
        let permitted = permitted_commands(SERVED_UI);
        let missing: Vec<_> = registered.difference(&permitted).collect();
        assert!(
            missing.is_empty(),
            "these commands are registered but not permitted for the served UI, so \
             invoking them fails at runtime with \"not allowed by ACL\": {missing:?}"
        );
    }

    #[test]
    fn every_registered_command_is_reachable_from_the_splash() {
        let registered = registered_commands();
        let permitted = permitted_commands(APP_LOCAL);
        let missing: Vec<_> = registered.difference(&permitted).collect();
        assert!(missing.is_empty(), "not permitted for the local splash: {missing:?}");
    }

    /// A permission for a command that no longer exists is dead weight,
    /// and reads as though the command is still there.
    #[test]
    fn no_capability_names_a_command_that_does_not_exist() {
        let registered = registered_commands();
        for (name, capability) in [("served-ui", SERVED_UI), ("app-local", APP_LOCAL)] {
            let stale: Vec<_> = permitted_commands(capability).difference(&registered).cloned().collect();
            assert!(stale.is_empty(), "{name} permits commands that are not registered: {stale:?}");
        }
    }

    /// The port is chosen at startup, so the grant has to cover any of
    /// them. A fixed port here would work until the first collision.
    #[test]
    fn the_remote_grant_covers_a_loopback_origin_on_any_port() {
        let json: serde_json::Value = serde_json::from_str(SERVED_UI).unwrap();
        let urls: Vec<&str> = json["remote"]["urls"].as_array().unwrap().iter().map(|u| u.as_str().unwrap()).collect();
        assert!(urls.iter().any(|u| u.starts_with("http://127.0.0.1:")), "no loopback grant: {urls:?}");
        assert!(
            urls.iter().all(|u| u.ends_with(":*")),
            "a fixed port would break as soon as it is taken: {urls:?}"
        );
    }
}
