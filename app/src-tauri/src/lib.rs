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

mod server;
mod settings;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::DialogExt;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerState::default())
        .invoke_handler(tauri::generate_handler![ping, get_persisted_library, choose_library])
        .setup(|app| {
            // Auto-open the last library, if any, without waiting for
            // the frontend to ask -- real startup UX, not just a
            // technically-correct command surface.
            if let Some(path) = settings::load_library_path(app.handle()) {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    if let Err(e) = open_library(&handle, path) {
                        eprintln!("failed to reopen the last library on startup: {e}");
                    }
                });
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
