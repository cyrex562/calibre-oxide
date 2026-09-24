# Building and running on Windows

Development happens on Linux, so Windows is the less-travelled path. This
page records what is known to be needed, and — just as importantly — what
has and has not actually been verified.

## Status

The workspace **builds and passes its tests on a real Windows host with
the real MSVC toolchain** — 5,210 tests, verified by
`.github/workflows/windows.yml` on `windows-latest`. That job runs on
every push and pull request, and is the thing to trust over this page.

Getting there took fixing seven genuine Windows defects, three of which
could have cost a user data. None were reachable from a Linux development
machine, which is the whole argument for that job.

What is still unverified is the app actually *running*: CI builds and
tests, it does not launch a window. Starting up, rendering, and spawning
`calibre_srv` all still want a human on a real desktop.

## Prerequisites

| | Why |
|---|---|
| [Rust](https://rustup.rs/) (stable, MSVC) | `rustup`'s Windows default is already `x86_64-pc-windows-msvc` — the right one |
| **Visual Studio Build Tools 2022**, "Desktop development with C++" | Several dependencies are C/C++ and compile from source: `libsqlite3-sys`, `zstd-sys`, `bzip2-sys`, `blake3`, `unrar_sys`, `libchm`, `espeak-ng` |
| [Node.js](https://nodejs.org/) 22+ | Builds the `web/` and `app/` frontends |
| WebView2 runtime | What Tauri renders in. Ships with Windows 11; on Windows 10 install the [Evergreen runtime](https://developer.microsoft.com/microsoft-edge/webview2/) |

NASM is *not* required. `ring` ships pre-assembled objects and only reaches
for NASM when built from a git checkout, which is not how Cargo consumes it
here. If a build error ever does mention NASM, install it and please open an
issue — that would mean something changed.

## Build and run

The app spawns `calibre_srv` as a child process and points it at the built
`web/` frontend, so both have to exist before the app will start. It looks
for a bundled resource first and falls back to the workspace's own
`target/` and `web/dist`.

**The order below is mandatory, not advisory.** `tauri build` declares
`calibre_srv.exe` and `web/dist` as bundle resources, and a declared
resource that does not exist is a hard bundle error — so steps 1 and 2
have to have happened first. That is deliberate: the alternative is a
packaged app that builds happily and then cannot find its own backend at
runtime.

```powershell
git clone https://github.com/cyrex562/calibre-oxide.git
cd calibre-oxide

cargo xtask build
```

That runs the three steps in order — Rust workspace, web UI, desktop
app — which is the whole reason it exists: getting the order wrong
fails somewhere that does not mention the order. It ends at a runnable
`calibre_oxide_app.exe`.

`cargo xtask package` additionally produces the MSI and NSIS
installers; `cargo xtask help` lists the rest.

By hand, if you want only one of them:

```powershell
cargo build --release --workspace        # includes calibre_srv, which the app spawns
cd web  ; npm install ; npm run build    # the UI the app displays
cd ..\app ; npm install ; npm run tauri:build
```

On first launch the app asks for a library folder and remembers it.

## Installing over an existing version

The MSI is a major-upgrade installer: running a newer one replaces the
installed version in place, with no need to uninstall first.

Two things make that safe, and both are deliberate:

**The upgrade code is pinned.** Windows identifies an existing
installation by its MSI `UpgradeCode`, and Tauri otherwise *derives*
one from the product name — so renaming the product would silently turn
future installers into a second app alongside the first rather than an
upgrade. `tauri.conf.json` pins it to the value already derived for
`calibre-oxide`, which is why installs made before it was pinned still
upgrade cleanly rather than duplicating.

**No user data lives in the install directory.** Nothing here writes
beside its own executable. Libraries live wherever you put them, and
everything else — settings, reader profiles, saved searches, the render
cache — lives under `%APPDATA%` and `%LOCALAPPDATA%`, which the
installer never touches. Uninstalling removes the program and leaves
the library alone.

One consequence worth knowing: MSI upgrades key off the version number,
so installing the *same* version over itself is not an upgrade —
Windows offers repair or remove instead. Bump `version` in
`tauri.conf.json` for a release meant to install over an older one.

## Running the tests

```powershell
cargo xtask test
```

which runs the Rust suite and the web suite. By hand:

```powershell
cargo test --workspace --lib
cd web ; npm test
```

Use `--lib`. Some `tests/` files in `calibre_db` are stale and fail for
reasons unrelated to any current work.

One test is deliberately skipped on Windows:
`save_to_disk`'s `a_symlink_planted_inside_dest_cannot_be_used_to_escape_it`.
Creating a directory symlink on Windows requires Developer Mode or
`SeCreateSymbolicLinkPrivilege`, so the test cannot build its own fixture.
The guard it covers is portable; only the fixture is not — which does mean
that particular security property goes unverified on Windows.

## Troubleshooting

### `LINK : fatal error LNK1104: cannot open file ...exe`

The linker could not write its own output, because something else had
that file open. Nothing to do with the code — the same commit links
fine in CI.

In order of likelihood:

1. **Two binaries with the same normalised name.** Cargo turns `-` into
   `_` for the intermediate artifact in `target/*/deps/`, so bin targets
   named `foo-bar` and `foo_bar` — even in different crates — link to
   the same path, and two linkers racing for one output file is exactly
   this error. This bit us once for real: `ebook-convert` and
   `ebook_convert` both existed, and it failed on CI and on a user's
   machine while being completely silent on Linux. If you add a binary,
   check `cargo metadata` for a normalised-name clash before assuming
   the environment is at fault.

2. **Windows Defender's real-time scanner** opens each freshly written
   `.exe` to scan it, and the linker can lose that race. This is the
   usual cause on a Windows Rust build. Exclude the build directory
   (PowerShell as Administrator):

   ```powershell
   Add-MpPreference -ExclusionPath "<repo>\target"
   ```

3. **A previous copy is still running.** The binary in the message names
   the process to look for:

   ```powershell
   Get-Process ebook-convert -ErrorAction SilentlyContinue | Stop-Process
   ```

4. **Another `cargo` is building the same workspace** in a second
   terminal, or a `tauri build` is running concurrently. Cargo locks its
   own target directory, but a `tauri build` invoking a nested cargo can
   still overlap with a manual one.

Simply re-running the build frequently succeeds, since the race is
timing-dependent — but if it happens more than once, do the exclusion
rather than keep retrying.

## Known gaps

- **Text-to-speech is untested on Windows.** `ort` (ONNX Runtime) does ship
  prebuilt binaries for `windows-msvc`, so it is expected to work, but
  nobody has run it.
- **Platform integration is Linux-only by design**, and degrades rather than
  breaks: device-removal and sleep/resume monitors (#258), "Open With"
  application discovery, and the system font directories all fall back to
  conservative defaults elsewhere.

## Cross-checking from Linux

Useful for catching a portability break without leaving a Linux box. This
is a *different toolchain* from the one above, so a pass here is a strong
hint, not proof.

```bash
sudo apt-get install -y mingw-w64
rustup target add x86_64-pc-windows-gnu

# mingw ships some headers lowercase, and Linux filesystems are
# case-sensitive, so unrar_sys cannot find them under the names it uses.
for d in /usr/share/mingw-w64/include /usr/x86_64-w64-mingw32/include; do
  sudo ln -sf powrprof.h "$d/PowrProf.h"
  sudo ln -sf wbemidl.h  "$d/Wbemidl.h"
done

# `ort` publishes no prebuilt binaries for the gnu target. Nothing here
# calls into it at check time, so point it at an empty directory.
export ORT_PREFER_DYNAMIC_LINK=1 ORT_SKIP_DOWNLOAD=1 ORT_LIB_LOCATION=/tmp/ort
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar

cargo check --workspace --all-targets --target x86_64-pc-windows-gnu
```

Use `--all-targets`. Two of the three breaks this setup first found were in
test code, and a plain `cargo check` would have missed both.
