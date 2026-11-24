//! Lexmechanic desktop app — Tauri backend.
//!
//! This app is a native shell around the real, already-working
//! `web/` browser UI and `lexmechanic_srv` backend (issue: real desktop
//! app + E2E testing plan) rather than a separate frontend talking to
//! `lexmechanic_db`/`lexmechanic_ebooks` in-process. On startup (or once a
//! library is chosen), this spawns the compiled `lexmechanic_srv` binary
//! pointed at the chosen library with `--static-dir` set to the built
//! `web/dist`, waits for it to be ready, and navigates the window to
//! it. From that point on, the real `web/` UI (library browsing,
//! reading, add/edit) talks to that spawned `lexmechanic_srv` exactly the
//! way it already does when served directly -- no new frontend code,
//! no duplicated UI.
//!
//! `ping` remains as a minimal smoke-test command from the earlier
//! scaffold; the real work now lives in [`server`] (spawn/health-
//! check/shutdown, binary/asset resolution) and [`settings`]
//! (persisted library path).

mod data_migration;
mod folder_check;
mod library_import;
mod logs;
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
    format!("Lexmechanic backend v{} online", env!("CARGO_PKG_VERSION"))
}

/// Holds the spawned `lexmechanic_srv` child process (if any) and the
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

/// Spawns `lexmechanic_srv` for `library_path`, waits for it to be ready,
/// and navigates the main window to it. Shared by app startup (a
/// previously-persisted library) and the "choose library" flow (a
/// freshly-picked one).
fn open_library(app: &AppHandle, library_path: std::path::PathBuf) -> Result<(), String> {
    // A failure from an earlier attempt must not be shown as this one's.
    *app.state::<logs::LaunchFailureState>().0.lock().unwrap() = None;
    let result = start_library(app, library_path);
    // Whatever the outcome is, the splash can ask for it: a start that
    // fails on a background thread (the automatic reopen at launch) has no
    // caller to return the error to.
    let dir = logs::log_dir(app);
    let failures = app.state::<logs::LaunchFailureState>();
    match &result {
        Ok(()) => *failures.0.lock().unwrap() = None,
        Err(message) => {
            logs::append_app_log(&dir, &format!("could not open library: {message}"));
            *failures.0.lock().unwrap() = Some(logs::LaunchFailure::new(message.clone(), &dir));
        }
    }
    result
}

fn start_library(app: &AppHandle, library_path: std::path::PathBuf) -> Result<(), String> {
    let state = app.state::<ServerState>();
    state.kill();
    let log_dir = logs::log_dir(app);

    let bin = server::resolve_calibre_srv_binary(app).map_err(|e| e.to_string())?;
    let static_dir = server::resolve_web_dist(app).map_err(|e| e.to_string())?;
    let port = server::find_free_port().map_err(|e| e.to_string())?;

    let pdfium = server::resolve_pdfium(app);
    // Every library this user has opened before is served alongside the
    // current one, so `library_id` in a URL addresses something real and
    // copy-to-library has somewhere to copy to (#959).
    let also_serve = settings::list_recent_libraries(app);
    // A log that cannot be opened must not stop the library opening.
    let log = logs::rotate_server_log(&log_dir).map_err(|e| logs::append_app_log(&log_dir, &format!("cannot open server log: {e}"))).ok();
    let mut child = server::spawn(&bin, &library_path, &static_dir, port, pdfium.as_deref(), &also_serve, log).map_err(|e| format!("could not start {}: {e}", bin.display()))?;

    // Opening a large library (a NAS-sized metadata.db, a change-log replay)
    // can take well over ten seconds before the server binds its port.
    let ready_within = server::ready_timeout();
    let readiness = server::wait_until_ready(port, ready_within, || child.try_wait().ok().flatten().map(|status| status.to_string()));
    match readiness {
        server::Readiness::Ready => {}
        server::Readiness::Exited(how) => {
            let _ = child.wait();
            return Err(format!("lexmechanic_srv stopped while starting ({how})"));
        }
        server::Readiness::TimedOut => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("lexmechanic_srv did not become ready on port {port} within {}s", ready_within.as_secs()));
        }
    }
    *state.0.lock().unwrap() = Some((child, port));

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
    let _ = window.set_title(&format!("Lexmechanic — {name}"));

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
/// `lexmechanic_srv` for it and navigates the window there.
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
    // Not a library yet: ask before the server writes a database into it.
    if !folder_check::is_existing_library(&path) && !confirm(&app, "Create a new library?", folder_check::NEW_LIBRARY_PROMPT.to_string(), "Create library", "Cancel").await {
        return Ok(false);
    }
    if folder_check::is_on_network_drive(&path) && !confirm(&app, "Network drive", folder_check::network_warning(&path), "Open anyway", "Cancel").await {
        return Ok(false);
    }
    open_library(&app, path)?;
    Ok(true)
}

/// A native OK/Cancel question; `true` only for the OK button.
async fn confirm(app: &AppHandle, title: &str, message: String, ok: &str, cancel: &str) -> bool {
    use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind};
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().message(message).title(title).kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::OkCancelCustom(ok.to_string(), cancel.to_string())).show(move |yes| {
        let _ = tx.try_send(yes);
    });
    rx.recv().await.unwrap_or(false)
}

/// Why the last attempt to start a library failed, with the tail of the
/// server's log, or `None` if the last attempt worked. The splash asks
/// for this while it waits, because a start that fails in the background
/// (reopening the last library at launch) has no caller to return to.
#[tauri::command]
fn get_launch_failure(state: State<'_, logs::LaunchFailureState>) -> Option<logs::LaunchFailure> {
    state.0.lock().unwrap().clone()
}

/// Opens the folder holding `server.log` and `app.log`.
#[tauri::command]
fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = logs::log_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Real whole-library import (issue #761): picks a real `.zip` archive
/// (the export side, `GET /library/export/{library_id}` on
/// `lexmechanic_srv`, produces exactly this shape -- a zip of the
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
/// switching means re-spawning `lexmechanic_srv` against a different
/// path, not serving several libraries at once.
#[tauri::command]
fn list_recent_libraries(app: AppHandle) -> Vec<String> {
    settings::list_recent_libraries(&app).into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// Re-opens a library from the recent-libraries list without going
/// through the folder-picker dialog again.
#[tauri::command]
async fn open_recent_library(app: AppHandle, path: String) -> Result<(), String> {
    // The page is a web page: whatever script runs in it can call this with
    // any string. Only a path the user has already opened (through the folder
    // picker, which is what puts it in the list) may be reopened this way.
    let recent = settings::list_recent_libraries(&app);
    let path = std::path::PathBuf::from(path);
    if !is_recent_library(&recent, &path) {
        return Err(format!("{} is not one of the recent libraries", path.display()));
    }
    open_library(&app, path)
}

/// Whether `path` names one of `recent`, compared as the same folder (case on
/// Windows, separators and a trailing slash don't matter).
fn is_recent_library(recent: &[std::path::PathBuf], path: &std::path::Path) -> bool {
    fn key(p: &std::path::Path) -> String {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let s = canonical.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    }
    let wanted = key(path);
    recent.iter().any(|r| key(r) == wanted)
}

/// Creates a new library and opens it.
///
/// Pointing `lexmechanic_srv` at an empty directory already produces a
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
    if folder_check::is_on_network_drive(&parent) && !confirm(&app, "Network drive", folder_check::network_warning(&parent), "Create anyway", "Cancel").await {
        return Ok(None);
    }

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

/// Real extensions `lexmechanic_ebooks::metadata::get_metadata`'s own
/// dispatch table understands (`crates/lexmechanic_ebooks/src/metadata/mod.rs`)
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
const BOOKS_ADDED_EVENT: &str = "lexmechanic:books-added";

#[derive(serde::Serialize, Default)]
struct AddFolderResult {
    added: u32,
    duplicates: Vec<String>,
    errors: Vec<String>,
    /// Exact source paths that were added as new books. Internal (not
    /// sent to the page): the auto-add watcher decides what to move
    /// aside from this, never by matching names in the messages above.
    #[serde(skip)]
    added_paths: Vec<std::path::PathBuf>,
}

/// The folder picker behind "Add folder".
///
/// Only *picks*: it returns the chosen folder's path (or `None` if the dialog
/// was cancelled) and the web UI's Add-folder dialog does the rest by posting
/// it to `POST /import-folder` with the user's choices -- one book per folder
/// or several, what to do about duplicates, recursive or not. That route is a
/// job with progress and cancel that copies the tree into the library,
/// collects every error instead of stopping at the first, and never modifies
/// the folder.
///
/// This used to be a flat, non-recursive loop here that uploaded each file
/// through `/cdb/add-book` one request at a time with no progress, titled
/// every book from its filename and gave up on nothing it could not read.
#[tauri::command]
async fn choose_import_folder(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    app.dialog().file().pick_folder(move |result| {
        let _ = tx.try_send(result);
    });
    let Some(Some(picked)) = rx.recv().await else {
        return Ok(None);
    };
    let dir = picked.into_path().map_err(|e| e.to_string())?;
    Ok(Some(dir.to_string_lossy().into_owned()))
}

/// A native folder picker for the web UI's "Export a copy" dialog: the chosen
/// folder's path, or `None` if the dialog was cancelled. It only picks; the
/// server plans and writes the copy.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let Some(dir) = pick_folder_path(&app).await else {
        return Ok(None);
    };
    Ok(Some(dir.to_string_lossy().into_owned()))
}

/// Uploads each file to the running `lexmechanic_srv`'s `/cdb/add-book`,
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
        // Streamed rather than read whole (#883). A dropped batch of
        // scanned PDFs used to cost roughly their combined size in
        // memory, on this side *and* the server's, before a byte moved.
        let file = match tokio::fs::File::open(path).await {
            Ok(file) => file,
            Err(e) => {
                result.errors.push(format!("{filename}: {e}"));
                continue;
            }
        };
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        let url = format!("http://127.0.0.1:{port}/cdb/add-book/{}/n/{}/-", simple_job_id(), urlencoding::encode(&filename));
        match client.post(&url).body(body).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body.get("book_id").is_some() => {
                    result.added += 1;
                    result.added_paths.push(path.clone());
                }
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
/// only (`is_viewable_format` in `lexmechanic_srv`), so without this a
/// PDF-first library has no way to open its own books.
///
/// The bytes come from the running `lexmechanic_srv` rather than from the
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

    let dir = std::env::temp_dir().join("lexmechanic-open");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // `book_id` is an integer and `ext` is alphanumeric-checked above,
    // so this name cannot escape `dir`.
    let path = dir.join(format!("{book_id}.{ext}"));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

// ===================================================================
// Revealing a library or a book in the system file browser (#886)
// ===================================================================
//
// Neither command takes a path from the page. The window displays
// content served over HTTP, which Tauri treats as remote for exactly
// this reason -- a command that opened any path it was handed would be
// a file-browser-anywhere primitive reachable from the page. Both
// resolve their own paths from state the Rust side already owns, and
// the book one additionally checks that what it resolved is still
// inside the library.

/// Opens `path` in the system file browser, after checking it is inside
/// `library`.
///
/// Both sides are canonicalised before comparing. A lexical
/// `starts_with` on unresolved paths cannot see a symlink planted
/// inside the library that points out of it -- the same check
/// `save_to_disk.rs` makes for its destination root, and for the same
/// reason.
fn open_inside_library(app: &AppHandle, library: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    let real_library = library.canonicalize().map_err(|e| format!("{}: {e}", library.display()))?;
    let real_path = path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?;
    if !real_path.starts_with(&real_library) {
        return Err(format!("{} is not inside the library", path.display()));
    }
    app.opener().open_path(real_path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Opens the library's own folder in the system file browser.
#[tauri::command]
fn reveal_library_folder(app: AppHandle) -> Result<(), String> {
    let library = settings::load_library_path(&app).ok_or("no library is currently open")?;
    // The library is trivially inside itself; going through the same
    // helper keeps the canonicalisation and the error text identical.
    open_inside_library(&app, &library.clone(), &library)
}

/// Opens the folder holding one book's files.
///
/// The book's folder is `<library>/<books.path>`; `/ajax/book/{id}` sends
/// `books.path` as `path` (relative, never the internal absolute
/// `fmt_<ext>` paths), so this needs no new route.
#[tauri::command]
async fn reveal_book_folder(state: State<'_, ServerState>, app: AppHandle, book_id: i32) -> Result<(), String> {
    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;
    let library = settings::load_library_path(&app).ok_or("no library is currently open")?;

    let relative = fetch_book_relative_path(port, book_id).await?;
    // An empty `path` means the book's files sit in the library root,
    // which is the normal case for a tracked folder (#889) and would
    // otherwise join to the library itself -- which is the right
    // answer, so no special case is needed beyond not rejecting it.
    let folder = library.join(relative.trim_start_matches(['/', '\\']));
    open_inside_library(&app, &library, &folder)
}

/// Finds one book's file for `ext` inside its folder.
///
/// `stem` is the book's own file name from `data.name`; the match is on that
/// name and the extension, both case-insensitively (a library can hold
/// `Book.PDF`). Matching on the extension alone picked whichever file the
/// directory listed first, which in a folder shared by several books is
/// usually another book's. With no `stem` (a server too old to send one) a
/// file is returned only when it is the folder's single file of that type.
/// `None` when it is not there, and the caller falls back to the folder.
fn find_format_file(folder: &std::path::Path, stem: Option<&str>, ext: &str) -> Option<std::path::PathBuf> {
    let with_ext: Vec<std::path::PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .collect();
    match stem {
        Some(stem) => with_ext.into_iter().find(|p| p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case(stem))),
        None if with_ext.len() == 1 => with_ext.into_iter().next(),
        None => None,
    }
}

/// Opens the system file browser with one book format selected.
///
/// Same trust model as [`reveal_book_folder`]: the page supplies only a
/// book id and a format name (checked by [`checked_ext`]); the path is
/// resolved here and must canonicalise to somewhere inside the library.
/// `reveal_item_in_dir` selects the file in Explorer / Finder and falls
/// back to opening the folder on desktops with no select support.
#[tauri::command]
async fn reveal_book_file(state: State<'_, ServerState>, app: AppHandle, book_id: i32, fmt: String) -> Result<(), String> {
    let port = state.0.lock().unwrap().as_ref().map(|(_, p)| *p).ok_or("no library is currently open")?;
    let library = settings::load_library_path(&app).ok_or("no library is currently open")?;
    let ext = checked_ext(&fmt)?;

    let (relative, stems) = fetch_book_location(port, book_id).await?;
    let folder = library.join(relative.trim_start_matches(['/', '\\']));
    let Some(file) = find_format_file(&folder, stems.get(&ext).map(String::as_str), &ext) else {
        // The file is not where the database says: show the folder instead of
        // guessing at another book's file.
        return open_inside_library(&app, &library, &folder);
    };

    let real_library = library.canonicalize().map_err(|e| format!("{}: {e}", library.display()))?;
    let real_file = file.canonicalize().map_err(|e| format!("{}: {e}", file.display()))?;
    if !real_file.starts_with(&real_library) {
        return Err(format!("{} is not inside the library", file.display()));
    }
    app.opener().reveal_item_in_dir(&real_file).map_err(|e| e.to_string())
}

#[cfg(test)]
mod reveal_tests {
    use super::{find_format_file, is_recent_library};
    use std::path::PathBuf;

    #[test]
    fn finds_format_file_case_insensitively() {
        let dir = std::env::temp_dir().join(format!("lexmechanic-reveal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Book.PDF"), b"x").unwrap();
        std::fs::write(dir.join("Book.epub"), b"x").unwrap();
        assert_eq!(find_format_file(&dir, Some("book"), "pdf").unwrap().file_name().unwrap(), "Book.PDF");
        assert!(find_format_file(&dir, Some("Book"), "mobi").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder shared by several books: the right one is picked by name, not
    /// by being first in the listing.
    #[test]
    fn picks_the_books_own_file_in_a_shared_folder() {
        let dir = std::env::temp_dir().join(format!("lexmechanic-reveal-shared-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["Aardvark.pdf", "Mine.pdf", "Zebra.pdf"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert_eq!(find_format_file(&dir, Some("Mine"), "pdf").unwrap().file_name().unwrap(), "Mine.pdf");
        assert!(find_format_file(&dir, Some("Missing"), "pdf").is_none(), "a missing file is None, never a neighbour's");
        assert!(find_format_file(&dir, None, "pdf").is_none(), "with no name, an ambiguous folder gives nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_recent_library_may_be_reopened() {
        let recent = vec![PathBuf::from("/books/one"), PathBuf::from("/books/two")];
        assert!(is_recent_library(&recent, &PathBuf::from("/books/two")));
        assert!(is_recent_library(&recent, &PathBuf::from("/books/two/")));
        assert!(!is_recent_library(&recent, &PathBuf::from("/etc")));
        assert!(!is_recent_library(&recent, &PathBuf::from("/books")));
        assert!(!is_recent_library(&[], &PathBuf::from("/books/one")));
    }
}

/// Reads one book's `path` column through the running server.
async fn fetch_book_relative_path(port: u16, book_id: i32) -> Result<String, String> {
    fetch_book_location(port, book_id).await.map(|(path, _)| path)
}

/// One book's `path` column and each format's file stem (`data.name`, keyed
/// by lowercase format), through the running server.
async fn fetch_book_location(port: u16, book_id: i32) -> Result<(String, std::collections::HashMap<String, String>), String> {
    let url = format!("http://127.0.0.1:{port}/ajax/book/{book_id}");
    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("no book with id {book_id}"));
    }
    let row: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    let path = row.get("path").and_then(|v| v.as_str()).map(str::to_string).ok_or_else(|| format!("book {book_id} has no folder recorded"))?;
    let stems = row
        .get("format_file_names")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.to_lowercase(), s.to_string()))).collect())
        .unwrap_or_default();
    Ok((path, stems))
}

/// Fetches one of a book's formats from the running `lexmechanic_srv`.
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

async fn pick_folder_path(app: &AppHandle) -> Option<std::path::PathBuf> {
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
/// `lexmechanic_ebooks::tweak::explode` leaves a hidden marker file
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

    let Some(parent) = pick_folder_path(&app).await else {
        return Ok(None);
    };

    let bytes = fetch_book_format(port, book_id, &ext).await?;

    let tmp = std::env::temp_dir().join(format!("lexmechanic-unpack-{book_id}.{ext}"));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;

    let dest = parent.join(format!("book-{book_id}-{ext}"));
    if dest.exists() {
        return Err(format!("{} already exists -- choose a different folder", dest.display()));
    }

    // The `question` callback answers the joint MOBI6+KF8 prompt.
    // Answering yes is right here: the user explicitly asked to
    // unpack this book, and declining would silently do nothing.
    lexmechanic_ebooks::tweak::explode(&tmp, &dest, |_| true).map_err(|e| e.to_string())?.ok_or_else(|| "that book could not be unpacked".to_string())?;
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

    let Some(dir) = pick_folder_path(&app).await else {
        return Ok(false);
    };

    let rebuilt = std::env::temp_dir().join(format!("lexmechanic-repack-{book_id}.{ext}"));
    let _ = std::fs::remove_file(&rebuilt);
    // `implode` verifies its own marker file, so a folder that was
    // not produced by unpacking -- or was unpacked from a different
    // format -- is refused here rather than producing a broken book.
    lexmechanic_ebooks::tweak::implode(&dir, &rebuilt).map_err(|e| e.to_string())?;

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

/// Subfolder of the drop folder that receives files once they have been
/// added. It is a directory, so `auto_add_candidates` (which only looks
/// at plain files directly in the folder) never picks it up again.
const AUTO_ADD_DONE_DIR: &str = ".added";

/// Moves an added source file into `<drop folder>/.added/`, returning
/// where it went. A name collision is resolved by renaming (`a (1).epub`),
/// never by overwriting, so no file is ever lost.
fn move_to_added(drop_folder: &std::path::Path, source: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let dest_dir = drop_folder.join(AUTO_ADD_DONE_DIR);
    std::fs::create_dir_all(&dest_dir)?;
    let name = source.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let stem = std::path::Path::new(name).file_stem().unwrap_or(name).to_string_lossy().into_owned();
    let ext = std::path::Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 0u32;
    loop {
        let candidate = if n == 0 { dest_dir.join(name) } else { dest_dir.join(format!("{stem} ({n}){ext}")) };
        // `hard_link` fails if the target exists, so this claims the name
        // atomically; a plain exists()+rename would race.
        match std::fs::hard_link(source, &candidate) {
            Ok(()) => {
                std::fs::remove_file(source)?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(_) => {
                // No hard links here (some network shares): fall back to
                // a rename after checking the name is free.
                if !candidate.exists() {
                    std::fs::rename(source, &candidate)?;
                    return Ok(candidate);
                }
                n += 1;
            }
        }
    }
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
    let Some(folder) = pick_folder_path(&app).await else {
        return Ok(settings::get_auto_add_folder(&app).map(|p| p.to_string_lossy().into_owned()));
    };
    settings::set_auto_add_folder(&app, Some(folder.clone())).map_err(|e| e.to_string())?;
    Ok(Some(folder.to_string_lossy().into_owned()))
}

/// Whether `folder` is the library itself or sits inside it.
///
/// Decides what the watcher does with what it finds, so it has to be
/// right about symlinks: both sides are canonicalised, because a watched
/// folder that is a symlink into the library is inside it however
/// different the two paths look. If either side cannot be canonicalised
/// the answer is "outside", which is the conservative direction -- an
/// import leaves the original in place for the user to see, while a
/// wrong "inside" would skip importing a file that genuinely needed it.
fn folder_is_inside_library(folder: &std::path::Path, library: &std::path::Path) -> bool {
    let (Ok(folder), Ok(library)) = (folder.canonicalize(), library.canonicalize()) else {
        return false;
    };
    folder == library || folder.starts_with(&library)
}

/// Brings the library index up to date with its folder, in place.
///
/// Nothing is copied and nothing is deleted: the files are already in
/// the library, so there is nothing to import them *to*.
async fn scan_library_via_server(port: u16) -> Result<serde_json::Value, String> {
    // `wait=true`: the route starts a background job and answers at once
    // by default. The watcher wants the finished report (how many were
    // added), and it is its own loop, so holding this request open blocks
    // nothing else. The job itself is visible, and cancellable, in the page.
    let url = format!("http://127.0.0.1:{port}/scan-library/-?wait=true");
    let resp = reqwest::Client::new().post(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
}

/// Starts the background watcher.
///
/// What it does depends on where the watched folder is, and the
/// distinction matters because getting it wrong destroys files.
///
/// **Outside the library** it is a drop folder: each file is imported
/// and then **moved** into an `.added/` subfolder of the drop folder
/// (never deleted -- the OS Recycle Bin does not exist on network
/// shares, and an import is not proof the library copy is good).
/// Moving it out of the way is the point of a drop folder -- leaving it
/// would mean re-adding the same book on every scan, and de-duplicating
/// by name would break the moment someone renamed a file.
///
/// **Inside the library** it is not a drop folder at all, it is part of
/// the library (#889), and the same behaviour would be a bug twice over:
/// importing would copy a file that is already there, and removing the
/// source would delete the book's own file. Watching the library root
/// would delete every book it indexed. So a folder inside the library is
/// *scanned* instead -- the index catches up with the folder and nothing
/// on disk is touched. Re-adding is prevented by the records themselves,
/// plus the ignore list for files the user removed but kept (#896), which
/// is what made the delete-after-import trick unnecessary here.
fn spawn_auto_add_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tauri::async_runtime::spawn_blocking(|| std::thread::sleep(AUTO_ADD_INTERVAL)).await.ok();

            let Some(folder) = settings::get_auto_add_folder(&app) else { continue };
            let Some(port) = app.state::<ServerState>().0.lock().unwrap().as_ref().map(|(_, p)| *p) else { continue };

            let inside = settings::load_library_path(&app).is_some_and(|library| folder_is_inside_library(&folder, &library));

            let result = if inside {
                // A scan reads the whole library, not just the watched
                // folder -- cheaper than it sounds (a `stat` per file)
                // and it means a file moved anywhere in the library is
                // re-attached rather than duplicated.
                match scan_library_via_server(port).await {
                    Ok(scanned) => AddFolderResult {
                        added_paths: Vec::new(),
                        added: scanned["added"].as_array().map(|a| a.len() as u32).unwrap_or(0),
                        duplicates: Vec::new(),
                        errors: scanned["failed"].as_array().map(|f| f.iter().map(|e| format!("{}: {}", e["path"].as_str().unwrap_or_default(), e["error"].as_str().unwrap_or_default())).collect()).unwrap_or_default(),
                    },
                    Err(e) => AddFolderResult { added: 0, duplicates: Vec::new(), errors: vec![e], added_paths: Vec::new() },
                }
            } else {
                let files = auto_add_candidates(&folder);
                if files.is_empty() {
                    continue;
                }
                let result = add_files_via_server(port, &files).await;
                // Only what really landed is set aside, decided per exact
                // path: a failed or duplicate file stays put, so nothing
                // is lost silently.
                for path in &result.added_paths {
                    if let Err(e) = move_to_added(&folder, path) {
                        eprintln!("auto-add: could not move {} aside: {e}", path.display());
                    }
                }
                result
            };

            // Nothing changed: not worth waking the page every 20s.
            if result.added == 0 && result.errors.is_empty() && result.duplicates.is_empty() {
                continue;
            }

            if let Some(window) = app.get_webview_window("main") {
                page_event::dispatch(&window, BOOKS_ADDED_EVENT, &result);
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before Tauri resolves its config/log/web-view folders: they are named
    // after the bundle identifier, which changed with the project's name.
    data_migration::migrate_app_data();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(ServerState::default())
        .manage(logs::LaunchFailureState::default())
        .invoke_handler(tauri::generate_handler![ping, get_launch_failure, open_log_folder, get_persisted_library, choose_library, create_library, choose_import_folder, list_recent_libraries, open_recent_library, get_auto_reopen, set_auto_reopen, import_library_archive, set_menu_actions, open_book_format, unpack_book, repack_book, open_external_url, reveal_library_folder, reveal_book_folder, reveal_book_file, get_auto_add_folder, choose_auto_add_folder, pick_folder])
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
                // The web UI does not listen on lexmechanic_srv's
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
                        // open_library has already logged this and recorded it for the splash.
                        let _ = open_library(&handle, path);
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
        lexmechanic_ebooks::tweak::explode(&src, &exploded, |_| true).unwrap().expect("explode should succeed");
        assert!(exploded.join("content.opf").exists(), "the OPF should be on disk for external editing");

        // Edit it the way a user would, with their own tools.
        let chapter = exploded.join("chapter1.xhtml");
        let edited = std::fs::read_to_string(&chapter).unwrap().replace("Hello", "Goodbye");
        std::fs::write(&chapter, &edited).unwrap();

        let rebuilt = dir.path().join("rebuilt.epub");
        lexmechanic_ebooks::tweak::implode(&exploded, &rebuilt).unwrap();

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
        assert!(lexmechanic_ebooks::tweak::implode(&plain, &out).is_err());
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
    fn move_to_added_moves_the_file_and_keeps_it_out_of_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let src = write_with_age(dir.path(), "book.epub", 60);

        let dest = move_to_added(dir.path(), &src).unwrap();

        assert!(!src.exists());
        assert_eq!(dest, dir.path().join(".added").join("book.epub"));
        assert!(dest.exists());
        assert!(auto_add_candidates(dir.path()).is_empty());
    }

    #[test]
    fn move_to_added_never_overwrites_on_a_name_collision() {
        let dir = tempfile::tempdir().unwrap();
        for content in [b"first".as_slice(), b"second", b"third"] {
            let src = dir.path().join("book.epub");
            std::fs::write(&src, content).unwrap();
            move_to_added(dir.path(), &src).unwrap();
        }
        let done = dir.path().join(".added");
        assert_eq!(std::fs::read(done.join("book.epub")).unwrap(), b"first");
        assert_eq!(std::fs::read(done.join("book (1).epub")).unwrap(), b"second");
        assert_eq!(std::fs::read(done.join("book (2).epub")).unwrap(), b"third");
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
/// The window navigates to the `lexmechanic_srv` it spawns, so its page is
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

/// The watcher's whole behaviour turns on this: a folder inside the
/// library is scanned, one outside is drained. Answering "outside" for a
/// folder that is really inside would delete the books it found.
#[cfg(test)]
mod watched_folder_tests {
    use super::folder_is_inside_library;

    #[test]
    fn the_library_root_itself_is_inside() {
        let dir = tempfile::tempdir().unwrap();
        assert!(folder_is_inside_library(dir.path(), dir.path()), "watching the library root must not be read as a drop folder -- it would delete every book it indexed");
    }

    #[test]
    fn a_subfolder_of_the_library_is_inside() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("Inbox");
        std::fs::create_dir(&inbox).unwrap();
        assert!(folder_is_inside_library(&inbox, dir.path()));
    }

    #[test]
    fn a_deeply_nested_subfolder_is_inside() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("Manuals/Boilers/New");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(folder_is_inside_library(&deep, dir.path()));
    }

    #[test]
    fn an_unrelated_folder_is_outside() {
        let library = tempfile::tempdir().unwrap();
        let downloads = tempfile::tempdir().unwrap();
        assert!(!folder_is_inside_library(downloads.path(), library.path()));
    }

    /// The library's *parent* is not inside it. A drop folder that
    /// happens to contain the library is still a drop folder.
    #[test]
    fn the_librarys_parent_is_outside() {
        let parent = tempfile::tempdir().unwrap();
        let library = parent.path().join("Books");
        std::fs::create_dir(&library).unwrap();
        assert!(!folder_is_inside_library(parent.path(), &library));
    }

    /// A sibling whose name merely starts with the library's. Comparing
    /// paths as strings would call `/tmp/lib-extra` a child of `/tmp/lib`.
    #[test]
    fn a_sibling_with_a_similar_name_is_outside() {
        let parent = tempfile::tempdir().unwrap();
        let library = parent.path().join("Books");
        let sibling = parent.path().join("Books-inbox");
        std::fs::create_dir(&library).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        assert!(!folder_is_inside_library(&sibling, &library), "a name prefix is not containment");
    }

    /// A symlink pointing into the library is inside it, however
    /// different the two paths look. Missing this would drain a folder
    /// whose files are the library's own.
    #[cfg(unix)]
    #[test]
    fn a_symlink_into_the_library_is_inside() {
        let library = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let real = library.path().join("Inbox");
        std::fs::create_dir(&real).unwrap();
        let link = elsewhere.path().join("watched");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(folder_is_inside_library(&link, library.path()), "a symlink into the library points at the library's own files");
    }

    /// And the converse: a symlink *out* of the library is outside it,
    /// so a folder reachable through the library tree is still drained.
    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_library_is_outside() {
        let library = tempfile::tempdir().unwrap();
        let downloads = tempfile::tempdir().unwrap();
        let link = library.path().join("inbox");
        std::os::unix::fs::symlink(downloads.path(), &link).unwrap();

        assert!(!folder_is_inside_library(&link, library.path()));
    }

    /// A folder that is not there cannot be canonicalised. "Outside" is
    /// the safe answer: an import leaves the original where the user can
    /// see it, whereas a wrong "inside" silently skips it.
    #[test]
    fn a_missing_folder_is_treated_as_outside() {
        let library = tempfile::tempdir().unwrap();
        assert!(!folder_is_inside_library(&library.path().join("gone"), library.path()));
    }
}
