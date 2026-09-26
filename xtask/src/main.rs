//! Build automation, run as `cargo xtask <command>`.
//!
//! Producing the desktop app takes three steps in two languages, and
//! they have to happen in order: the Rust workspace builds
//! `calibre_srv`, `web/` builds the UI the app displays, and only then
//! can `tauri build` package the two together. Getting that order wrong
//! fails somewhere unhelpful, so it lives here instead of in a README
//! that nobody reads twice.
//!
//! The `xtask` pattern is a plain binary rather than a build script or a
//! shell script: it is cross-platform without a shell, it is type
//! checked, and `cargo xtask` needs no tooling a contributor does not
//! already have. See <https://github.com/matklad/cargo-xtask>.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

fn main() {
    if let Err(e) = run() {
        // `{:#}` so the `.context(...)` chain is visible -- "npm install
        // failed in web/" is worth having above "exited with status 1".
        eprintln!("\nxtask: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (task, rest) = args.split_first().map(|(t, r)| (t.as_str(), r)).unwrap_or(("help", &[][..]));
    let release = !rest.iter().any(|a| a == "--debug");

    match task {
        "build" => build_all(release),
        "app" => build_app(release),
        "server" => build_rust(release),
        "web" => build_web(),
        "package" => package(),
        "fetch-pdfium" => fetch_pdfium(),
        "test" => test_all(),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            print_help();
            bail!("unknown task {other:?}");
        }
    }
}

fn print_help() {
    eprintln!(
        "\ncargo xtask <task>\n\n\
         \x20 build     everything, in order: Rust workspace, web UI, then the desktop app\n\
         \x20 server    just the Rust workspace (includes calibre_srv and the CLIs)\n\
         \x20 web       just the web UI that the app displays\n\
         \x20 app       the desktop app, assuming the two above are already built\n\
         \x20 package   build everything, then produce installers (needs network)\n\
         \x20 test      the Rust test suite and the web test suite\n\
         \x20 fetch-pdfium  download the PDF rendering library (needed for PDF covers)\n\n\
         \x20 --debug   build unoptimized (default is release)\n"
    );
}

/// The repo root, found from this crate rather than the current
/// directory, so `cargo xtask` works from anywhere in the tree.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask/ always has a parent").to_path_buf()
}

fn build_all(release: bool) -> Result<()> {
    build_rust(release)?;
    build_web()?;
    build_app(release)
}

fn build_rust(release: bool) -> Result<()> {
    step("building the Rust workspace");
    let mut args = vec!["build", "--workspace"];
    if release {
        args.push("--release");
    }
    cargo(&args, &root()).context("building the Rust workspace")
}

fn build_web() -> Result<()> {
    step("building the web UI");
    let web = root().join("web");
    npm(&["install"], &web).context("npm install failed in web/")?;
    npm(&["run", "build"], &web).context("npm run build failed in web/")
}

fn build_app(release: bool) -> Result<()> {
    step("building the desktop app");
    let app = root().join("app");
    npm(&["install"], &app).context("npm install failed in app/")?;
    // `tauri build` is release by definition; a debug app build is
    // `tauri dev`, which is interactive and not what this is for.
    if !release {
        eprintln!("  note: --debug does not apply to the desktop app; `tauri build` is always optimized");
    }
    // Deliberately `--no-bundle`: this produces a runnable app and
    // nothing else. Generating installers is `package`, and it is kept
    // separate because bundling is the part that can fail for reasons
    // having nothing to do with the code -- the Linux AppImage bundler
    // downloads its tooling from GitHub at build time, so `build` would
    // otherwise need working network to a specific host to finish.
    npm(&["run", "tauri:build"], &app).context("building the desktop app failed")?;
    let exe = root().join("target").join("release").join(if cfg!(windows) { "calibre_oxide_app.exe" } else { "calibre_oxide_app" });
    eprintln!("\n  built {}", exe.display());
    Ok(())
}

fn package() -> Result<()> {
    build_rust(true)?;
    build_web()?;
    step("packaging installers");
    let app = root().join("app");
    npm(&["install"], &app).context("npm install failed in app/")?;
    npm(&["run", "tauri:package"], &app).context(
        "packaging failed -- note that the Linux AppImage bundler downloads its tooling \
         from GitHub while it runs, so this step needs network access that `build` does not",
    )?;
    let bundle = root().join("target").join("release").join("bundle");
    step(&format!("installers are in {}", bundle.display()));
    Ok(())
}

/// Downloads PDFium and puts it where the built binaries will find it.
///
/// # Why this is a separate command and not part of `build`
///
/// PDF page rendering -- covers from page 1, exporting a page as an
/// image -- needs Google's PDFium, a C++ library with no pure-Rust
/// equivalent. It is bound at *run* time (`crates/calibre_ebooks/src/
/// pdf/rasterize.rs` explains why), so nothing here is needed to
/// compile or test the workspace: without it, PDFs simply import
/// without covers.
///
/// Keeping it out of `build` is the point. `build` works offline; this
/// reaches GitHub. Folding a network download into the build step is
/// exactly what makes `ort` painful in this workspace.
///
/// # Why `curl` and `tar` rather than Rust crates
///
/// They avoid adding an HTTP stack, a gzip decoder and a tar reader to
/// a build tool that currently depends on `anyhow` alone. Both ship
/// with Windows 10 1803 and later as `curl.exe` and `tar.exe`, and are
/// standard on macOS and Linux, so this is not a shell script in
/// disguise -- it invokes two binaries directly, the same way the rest
/// of this file invokes `cargo` and `npm`.
fn fetch_pdfium() -> Result<()> {
    let asset = pdfium_asset_name()?;
    let url = format!("https://github.com/bblanchon/pdfium-binaries/releases/latest/download/{asset}");

    let root = root();
    let staging = root.join("target").join("pdfium");
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let archive = staging.join(&asset);

    step(&format!("downloading {asset}"));
    // `-f` so an HTTP error is a failure rather than a saved error
    // page; `-L` because the download URL is a redirect.
    exec(Command::new("curl").args(["-fsSL", "-o"]).arg(&archive).arg(&url), "curl").with_context(|| format!("downloading {url}"))?;

    step("extracting");
    exec(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&staging), "tar").context("extracting the PDFium archive")?;

    // The archive lays the library out the way the platform does:
    // `bin/pdfium.dll` on Windows, `lib/libpdfium.{so,dylib}`
    // elsewhere.
    let (subdir, lib_name) = if cfg!(windows) {
        ("bin", "pdfium.dll")
    } else if cfg!(target_os = "macos") {
        ("lib", "libpdfium.dylib")
    } else {
        ("lib", "libpdfium.so")
    };
    let extracted = staging.join(subdir).join(lib_name);
    if !extracted.exists() {
        bail!("{} is not in the downloaded archive -- the release layout may have changed", extracted.display());
    }

    // Both profiles: which one a developer runs is their business, and
    // the file is small enough that copying it twice is not worth
    // making them choose.
    let mut installed = Vec::new();
    for profile in ["debug", "release"] {
        let dir = root.join("target").join(profile);
        if !dir.exists() {
            continue;
        }
        let dest = dir.join(lib_name);
        std::fs::copy(&extracted, &dest).with_context(|| format!("copying to {}", dest.display()))?;
        installed.push(dest);
    }

    if installed.is_empty() {
        // Nothing has been built yet, so there is nowhere for it to
        // go that the loader would look. Say where it ended up rather
        // than silently succeeding.
        eprintln!(
            "\n  downloaded to {}\n  target/debug and target/release do not exist yet -- build first, then run this again",
            extracted.display()
        );
        return Ok(());
    }

    eprintln!();
    for path in &installed {
        eprintln!("  installed {}", path.display());
    }
    Ok(())
}

/// The release asset for the host platform.
fn pdfium_asset_name() -> Result<String> {
    let os = if cfg!(windows) {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        bail!("no prebuilt PDFium is published for this operating system");
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => bail!("no prebuilt PDFium is published for {other}"),
    };

    Ok(format!("pdfium-{os}-{arch}.tgz"))
}

fn test_all() -> Result<()> {
    step("running the Rust tests");
    // `--lib` deliberately: some `tests/` files in calibre_db are stale
    // and fail for reasons unrelated to any current change. The Windows
    // CI job uses the same scope.
    cargo(&["test", "--workspace", "--lib"], &root()).context("the Rust test suite failed")?;
    step("running the web tests");
    npm(&["install"], &root().join("web")).context("npm install failed in web/")?;
    npm(&["test"], &root().join("web")).context("the web test suite failed")
}

fn step(what: &str) {
    eprintln!("\n=== {what} ===");
}

fn cargo(args: &[&str], dir: &Path) -> Result<()> {
    // `CARGO` rather than a bare "cargo" so a toolchain override in
    // effect for this invocation stays in effect for the child.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    exec(Command::new(cargo).args(args).current_dir(dir), "cargo")
}

fn npm(args: &[&str], dir: &Path) -> Result<()> {
    // On Windows `npm` is `npm.cmd`, a batch file, which
    // `Command::new` will not find or execute on its own.
    let program = if cfg!(windows) { "npm.cmd" } else { "npm" };
    exec(Command::new(program).args(args).current_dir(dir), program)
}

fn exec(cmd: &mut Command, name: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("could not run {name} -- is it installed and on PATH?"))?;
    if !status.success() {
        bail!("{name} exited with {status}");
    }
    Ok(())
}
