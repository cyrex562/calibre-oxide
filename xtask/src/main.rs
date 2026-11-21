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
         \x20 test      the local merge gate: all Rust tests (unit, integration, e2e) and the web tests\n\
         \x20 fetch-pdfium  download the PDF rendering library (needed for PDF covers)\n\n\
         \x20 --debug   build unoptimized (default is release)\n"
    );
}

/// The repo root: the nearest directory at or above the **current working
/// directory** that holds a `Cargo.toml` with a `[workspace]` table and an
/// `xtask` directory, so `cargo xtask` works from anywhere in the tree.
///
/// Not `CARGO_MANIFEST_DIR`: that is baked in when xtask is compiled, and the
/// binary lives in the shared target directory, so whichever worktree built it
/// last would be "the repo" for every other (`fetch-pdfium` once downloaded
/// into a deleted worktree). The compile-time path is only the fallback, for
/// a run from outside any workspace.
fn root() -> PathBuf {
    std::env::current_dir().ok().and_then(|cwd| find_workspace_root(&cwd)).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask/ always has a parent").to_path_buf())
}

/// Walks up from `start` to the workspace root (see [`root`]).
fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join("xtask").is_dir() && std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|toml| toml.lines().any(|line| line.trim() == "[workspace]"))).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-root-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_root_is_found_from_a_subdirectory_of_whichever_tree_we_are_in() {
        for tree in ["one", "two"] {
            let dir = scratch(tree);
            std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
            std::fs::create_dir_all(dir.join("xtask")).unwrap();
            let deep = dir.join("crates").join("deep");
            std::fs::create_dir_all(&deep).unwrap();
            assert_eq!(find_workspace_root(&deep), Some(dir.clone()));
            assert_eq!(find_workspace_root(&dir), Some(dir.clone()));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_cargo_toml_without_workspace_or_without_xtask_is_not_the_root() {
        let dir = scratch("nested");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        // No xtask directory: not the root.
        assert_eq!(find_workspace_root(&dir), None);
        std::fs::create_dir_all(dir.join("xtask")).unwrap();
        let member = dir.join("crates").join("member");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(member.join("Cargo.toml"), "[package]\nname = \"m\"\n").unwrap();
        // A member's own manifest is skipped for the workspace above it.
        assert_eq!(find_workspace_root(&member), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_running_trees_root_is_the_workspace_it_is_in() {
        // `cargo test` runs from the package directory; the walk finds this repo.
        let found = find_workspace_root(&std::env::current_dir().unwrap()).expect("inside the workspace");
        assert!(found.join("xtask").join("Cargo.toml").is_file());
    }
}

fn build_all(release: bool) -> Result<()> {
    build_rust(release)?;
    build_web()?;
    build_app(release)
}

fn build_rust(release: bool) -> Result<()> {
    step("building the Rust workspace");
    // xtask itself is excluded: it is the running exe (`cargo xtask` runs
    // target/release/xtask.exe), and Windows refuses to replace a running
    // exe whenever cargo decides to rebuild it.
    let mut args = vec!["build", "--workspace", "--exclude", "xtask"];
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
    let target = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| root().join("target"));
    let bundle = target.join("release").join("bundle");
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

/// The local merge gate: every Rust test in the workspace (unit,
/// integration and the black-box `e2e/` crate), then the web suite.
///
/// Nothing stops at the first failure -- a full list of what is broken
/// is worth more than the first item -- and the exit status is non-zero
/// if anything at all failed.
///
/// Doctests are left out on purpose: they relink a separate binary per
/// snippet, which costs minutes, and the crates' examples are not where
/// regressions have shown up. Run them with `cargo test --workspace --doc`.
fn test_all() -> Result<()> {
    let started = std::time::Instant::now();
    let mut failures: Vec<String> = Vec::new();

    // The web UI has to be built for `e2e/` to exercise the real
    // server-plus-UI path; without `web/dist` those tests print "skipping"
    // and pass, which would make the gate lie.
    step("preparing the web UI");
    let web = root().join("web");
    let web_ready = npm(&["install"], &web).context("npm install failed in web/").and_then(|()| {
        if web.join("dist").join("index.html").is_file() {
            Ok(())
        } else {
            npm(&["run", "build"], &web).context("npm run build failed in web/")
        }
    });
    if let Err(e) = &web_ready {
        failures.push(format!("web: could not prepare ({e:#})"));
    }

    // `e2e/` spawns the compiled binaries (calibredb, calibre_srv, ...);
    // `cargo test` only builds the binaries of the package under test.
    step("building the workspace binaries (the e2e tests spawn them)");
    if let Err(e) = cargo(&["build", "--workspace", "--bins"], &root()) {
        failures.push(format!("building the workspace binaries failed ({e:#})"));
    }

    step("running the Rust tests (unit + integration + e2e)");
    let packages = target_packages();
    let (status_ok, per_crate) = run_cargo_tests(&packages);

    let web_tests = if web_ready.is_ok() {
        step("running the web tests");
        match npm(&["test"], &web) {
            Ok(()) => true,
            Err(e) => {
                failures.push(format!("web tests failed ({e:#})"));
                false
            }
        }
    } else {
        false
    };

    eprintln!("\n=== summary ===");
    let (mut passed, mut failed, mut ignored) = (0, 0, 0);
    let mut names: Vec<_> = per_crate.keys().collect();
    names.sort();
    for name in names {
        let c = &per_crate[name];
        eprintln!("  {name:<28} {:>5} passed {:>3} failed {:>3} ignored", c.passed, c.failed, c.ignored);
        passed += c.passed;
        failed += c.failed;
        ignored += c.ignored;
    }
    eprintln!("  {:<28} {passed:>5} passed {failed:>3} failed {ignored:>3} ignored", "TOTAL (rust)");
    eprintln!("  web tests: {}", if web_tests { "passed" } else { "FAILED or not run" });
    eprintln!("  elapsed: {:.0?}", started.elapsed());

    if !status_ok {
        failures.push(format!("the Rust test run reported failures ({failed} failed tests, or a binary that did not build or run)"));
    }
    if failures.is_empty() {
        eprintln!("\nALL TESTS PASSED");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("  FAILED: {f}");
        }
        bail!("{} part(s) of the test run failed", failures.len())
    }
}

#[derive(Default)]
struct Counts {
    passed: u32,
    failed: u32,
    ignored: u32,
}

/// Maps a cargo target name (as it appears in a test binary's file name,
/// dashes as underscores) to its package.
fn target_packages() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let Ok(out) = Command::new(cargo_bin).args(["metadata", "--no-deps", "--format-version", "1"]).current_dir(root()).output() else {
        return map;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return map };
    for pkg in v["packages"].as_array().into_iter().flatten() {
        let pname = pkg["name"].as_str().unwrap_or_default();
        for t in pkg["targets"].as_array().into_iter().flatten() {
            if let Some(n) = t["name"].as_str() {
                map.insert(n.replace('-', "_"), pname.to_string());
            }
        }
    }
    map
}

/// Pulls `N` out of `... N passed; ...` style parts of a `test result:` line.
fn count_before(line: &str, word: &str) -> u32 {
    line.split(';')
        .find_map(|part| {
            let part = part.trim();
            let part = part.rsplit(':').next().unwrap_or(part).trim();
            let part = part.trim_start_matches("ok.").trim_start_matches("FAILED.").trim();
            part.strip_suffix(word).and_then(|n| n.trim().parse().ok())
        })
        .unwrap_or(0)
}

/// Runs `cargo test`, echoing everything, while totting up each test
/// binary's `test result:` line under the package it belongs to.
/// Returns whether cargo exited successfully.
fn run_cargo_tests(packages: &std::collections::HashMap<String, String>) -> (bool, std::collections::HashMap<String, Counts>) {
    use std::io::BufRead;
    let mut per_crate: std::collections::HashMap<String, Counts> = std::collections::HashMap::new();
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    // One pipe for both streams so "Running <binary>" (stderr) stays
    // ahead of that binary's results (stdout).
    let Ok((reader, writer)) = std::io::pipe() else { return (false, per_crate) };
    let Ok(writer2) = writer.try_clone() else { return (false, per_crate) };
    let child = Command::new(cargo_bin)
        .args(["test", "--workspace", "--lib", "--bins", "--tests", "--no-fail-fast"])
        .current_dir(root())
        .stdout(writer)
        .stderr(writer2)
        .spawn();
    let Ok(mut child) = child else { return (false, per_crate) };

    let mut current = String::from("(unknown)");
    for line in std::io::BufReader::new(reader).lines().map_while(|l| l.ok()) {
        println!("{line}");
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("Running ") {
            // `Running unittests src/lib.rs (target\debug\deps\calibre_db-0123abcd.exe)`
            let exe = rest.rsplit(['\\', '/']).next().unwrap_or("").trim_end_matches(')');
            let stem = exe.trim_end_matches(".exe");
            let target = stem.rsplit_once('-').map(|(n, _)| n).unwrap_or(stem);
            current = packages.get(target).cloned().unwrap_or_else(|| target.to_string());
        } else if t.starts_with("test result:") {
            let entry = per_crate.entry(current.clone()).or_default();
            entry.passed += count_before(t, "passed");
            entry.failed += count_before(t, "failed");
            entry.ignored += count_before(t, "ignored");
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    (ok, per_crate)
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
