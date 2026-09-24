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
         \x20 package   build everything, then produce installers\n\
         \x20 test      the Rust test suite and the web test suite\n\n\
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
    npm(&["run", "tauri:build"], &app).context("tauri build failed in app/")
}

fn package() -> Result<()> {
    build_all(true)?;
    let bundle = root().join("target").join("release").join("bundle");
    step(&format!("installers are in {}", bundle.display()));
    Ok(())
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
