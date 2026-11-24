# Lexmechanic app

Tauri + Vue desktop shell for the Lexmechanic library manager. It is a
native window around the real library UI: the Rust backend under
`src-tauri/` spawns the `lexmechanic_srv` binary (the workspace's content
server) on a free loopback port, points it at the chosen library with
`--static-dir` set to the built `web/dist`, and navigates the window to
it. The library UI (`web/`) then talks to that server exactly as it does
in a browser. `src/` here is only the splash: choosing, creating and
reopening a library before the served UI takes over. The Tauri commands in
`src-tauri/` cover what a browser cannot do (folder pickers, opening a book
in the OS default app, revealing a folder).

## First-time setup

```bash
cd app
npm ci
```

You also need the Rust toolchain (workspace already uses it) plus, on
Windows, the WebView2 runtime that Tauri needs (already present on
Windows 11; the installer fetches it on a machine that lacks it).

## Dev

```bash
npm run tauri:dev
```

Starts Vite on `:1420`, waits for it, then launches the Tauri window
pointed at it. Hot-reload works for the Vue side; Cargo watches the
Rust side.

## Build the installer

```bash
cargo xtask fetch-pdfium     # once: PDF covers and fingerprints need it (downloads)
cargo xtask package          # release build, web UI, then the installer
```

Prerequisites: the Rust toolchain, Node and npm, PDFium (see below), and
on Windows the NSIS tooling Tauri caches under `%LOCALAPPDATA%\tauri\NSIS`
(Tauri downloads it on first use).

`cargo xtask package` builds `lexmechanic_srv` and `lexmechanic_pdf_worker` in
release mode, builds `web/`, then runs `npm run tauri:package`. That
runs `scripts/bundle-config.mjs`, which checks everything the installer
must carry and writes `src-tauri/tauri.bundle.conf.json` (it stops with a
list of what is missing instead of letting Tauri fail later):

- `lexmechanic_srv`, `lexmechanic_pdf_worker`, and `web/dist` (as `web-dist/`);
- PDFium, looked for in `$LEXMECHANIC_PDFIUM`, then `<target>/pdfium/bin`,
  then `<target>/release`. The build fails if it is in none of them;
- on Windows, every DLL the two exes import that Windows does not supply:
  today `DirectML.dll`, which the `ort` build leaves in `<target>/release`
  as a link into `%LOCALAPPDATA%\ort.pyke.io`. The link is resolved and the
  real file bundled, because without it Windows would load the older
  `System32\DirectML.dll`.

`<target>` is `CARGO_TARGET_DIR` if set, else `target/`.

Output is under `target/release/bundle/` (on Windows
`target/release/bundle/nsis/Lexmechanic_<version>_x64-setup.exe`). The
Windows installer is NSIS, installs per user (no administrator prompt),
and creates a Start menu shortcut and an uninstaller.

**Upgrading from calibre-oxide.** The product name and bundle identifier
changed with the rename, so the installer treats this as a different app:
it installs *beside* the old one and does not replace it. Uninstall the
old app (Settings, Apps) after installing this one. On its first start the
new app copies the old app's data (recent libraries, settings, window
state, logs, web-view storage) from `com.cyrex.calibre-oxide` to
`com.cyrex.lexmechanic` under `%APPDATA%` and `%LOCALAPPDATA%`; the old
folders are left in place. A library's own `.calibre-oxide/` state folder is
renamed to `.lexmechanic/` the first time the library is opened. Libraries
should be closed in the old app first (the rename fails, and says so, while
the old app still holds the folder open).

The installed app needs the Microsoft Visual C++ runtime
(`VCRUNTIME140`, `MSVCP140`; the "Visual C++ Redistributable" for
Visual Studio 2015-2022, x64). It is present on most machines, and it is
not bundled; install it on one that lacks it.

To run the bundle scripts' tests: `npm test`.

## Logs

The server's stdout and stderr go to `server.log` (the previous run is
kept as `server.prev.log`), and the app's own errors to `app.log`, in the
app log folder (on Windows `%APPDATA%\com.cyrex.lexmechanic\logs`). If a
library cannot be opened, the splash shows the error, the end of
`server.log`, and an "Open log folder" button.

## Opening a library

"Open a library" on a folder with no `metadata.db` asks before creating a
library there, and a library on a network drive gets a warning that
working on a local copy is recommended.

## Fault-tolerance contract

Every command that reads or writes library state must go through
`lexmechanic_db::LibraryHandle` (or equivalent for other subsystems). No
raw `std::fs` calls against a library path from this crate. See
`docs/FAULT_TOLERANCE.md`.

## Why not iced?

The old iced-based prototype lives at `app-iced-prototype/` for
reference. We switched to Tauri + Vue because iced's virtualized-table
widget is not production-grade for a Calibre-scale library view. The
user has good prior experience with Tauri + Vue on other projects.
