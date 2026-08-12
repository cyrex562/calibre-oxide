# calibre-oxide app

Tauri + Vue desktop shell for the calibre-oxide library manager. The
Rust backend under `src-tauri/` links directly to the workspace crates
(`calibre_db`, `calibre_ebooks`, `calibre_utils`) and exposes commands
via `#[tauri::command]`. The Vue front-end calls them with
`invoke("cmd_name", args)`.

## First-time setup

```bash
cd app
npm install
```

You also need the Rust toolchain (workspace already uses it) plus, on
Windows, the WebView2 runtime that Tauri needs (usually already
present on Windows 11).

## Dev

```bash
npm run tauri:dev
```

Starts Vite on `:1420`, waits for it, then launches the Tauri window
pointed at it. Hot-reload works for the Vue side; Cargo watches the
Rust side.

## Production build

```bash
npm run tauri:build
```

Bundles to `src-tauri/target/release/bundle/`.

## Fault-tolerance contract

Every command that reads or writes library state must go through
`calibre_db::LibraryHandle` (or equivalent for other subsystems). No
raw `std::fs` calls against a library path from this crate. See
`docs/FAULT_TOLERANCE.md`.

## Why not iced?

The old iced-based prototype lives at `app-iced-prototype/` for
reference. We switched to Tauri + Vue because iced's virtualized-table
widget is not production-grade for a Calibre-scale library view. The
user has good prior experience with Tauri + Vue on other projects.
