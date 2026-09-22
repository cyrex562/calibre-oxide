# Building and running on Windows

Development happens on Linux, so Windows is the less-travelled path. This
page records what is known to be needed, and — just as importantly — what
has and has not actually been verified.

## Status

The workspace **type-checks clean for Windows**, including the Tauri app.
That was established by cross-compiling from Linux to
`x86_64-pc-windows-gnu` with `--all-targets`.

It has **not** been built or run on a real Windows host yet. The
`.github/workflows/windows.yml` job exists to close exactly that gap: it
builds and tests on `windows-latest` with the real MSVC toolchain on every
push and pull request. Trust that job over this page.

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
`target/` and `web/dist` — which is what makes the development flow below
work without packaging anything.

```powershell
git clone https://github.com/cyrex562/calibre-oxide.git
cd calibre-oxide

# 1. The Rust side, including the calibre_srv binary the app spawns.
cargo build --workspace

# 2. The web UI the app actually displays.
cd web
npm install
npm run build
cd ..

# 3. The desktop app.
cd app
npm install
npm run tauri:dev
```

On first launch the app asks for a library folder and remembers it.

## Running the tests

```powershell
cargo test --workspace --lib
cd web; npm test
```

Use `--lib`. Some `tests/` files in `calibre_db` are stale and fail for
reasons unrelated to any current work.

One test is deliberately skipped on Windows:
`save_to_disk`'s `a_symlink_planted_inside_dest_cannot_be_used_to_escape_it`.
Creating a directory symlink on Windows requires Developer Mode or
`SeCreateSymbolicLinkPrivilege`, so the test cannot build its own fixture.
The guard it covers is portable; only the fixture is not — which does mean
that particular security property goes unverified on Windows.

## Known gaps

- **`tauri build` will not produce a working installer yet.** `tauri.conf.json`
  declares no `bundle.resources`, so a packaged app ships without
  `calibre_srv.exe` or `web/dist` and cannot find them at runtime. The
  development flow above is unaffected. Tracked separately.
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
