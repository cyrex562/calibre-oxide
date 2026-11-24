// Writes the bundle-only Tauri config used by `npm run tauri:package`.
//
// # Why this is generated, and why it is not in tauri.conf.json
//
// `bundle.resources` cannot live in `tauri.conf.json` (or in a
// `tauri.<platform>.conf.json`, which merges just as unconditionally).
// `tauri-build`'s build script validates every declared resource at
// *compile* time, so declaring them there makes a plain
// `cargo build --workspace` -- or `cargo test`, or `cargo check` --
// fail unless `web/dist` has already been built. That inverts the
// build order: the Rust build would depend on the frontend build, which
// is surprising for anyone working only on the Rust side, and it is
// circular against the documented steps.
//
// Passing this file to `tauri build --config` instead means the
// resources are only required at the one moment they actually have to
// exist: when a package is being produced.
//
// # Why it is generated rather than checked in
//
// The server binary carries a `.exe` on Windows and does not elsewhere,
// and a static JSON file cannot say "whichever of these two exists".
// Node already knows the platform, so it writes the right name. The same
// goes for where cargo put things (`CARGO_TARGET_DIR`), where PDFium is,
// and which DLLs the built exes import: all facts about this machine.

import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bundleResources, findPdfium, planDllBundle, platformNames, readExeImports } from "./bundle-resources.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const srcTauri = join(here, "..", "src-tauri");
const repoRoot = join(here, "..", "..");
const platform = process.platform;

// Cargo's target dir is not always `<repo>/target`: it is where
// CARGO_TARGET_DIR says, and a worktree or CI box commonly sets it.
const targetDir = resolve(process.env.CARGO_TARGET_DIR || join(repoRoot, "target"));
const releaseDir = join(targetDir, "release");

const { serverExe, workerExe, pdfiumLib } = platformNames(platform);

// Fail here, with a sentence that says what to run, rather than letting
// Tauri fail later with a bare "resource path doesn't exist".
const missing = [];
for (const exe of [serverExe, workerExe]) {
  // The worker is a second binary of lexmechanic_srv; without it the server
  // falls back to slow in-process PDFium, so it is required, not optional.
  if (!existsSync(join(releaseDir, exe))) {
    missing.push(`  ${exe} in ${releaseDir} -- run: cargo build --release --workspace`);
  }
}
if (!existsSync(join(repoRoot, "web", "dist", "index.html"))) {
  missing.push("  web/dist -- run: npm ci && npm run build   (in web/)");
}

// PDFium is required: without it the packaged app would import PDFs
// with no cover and no fingerprint, a feature that worked in the dev
// checkout silently gone. It is looked for in several places; if it is in
// none the build stops and says where it looked.
const pdfium = findPdfium({
  platform,
  envDir: process.env.LEXMECHANIC_PDFIUM ?? process.env.CALIBRE_OXIDE_PDFIUM,
  targetDirs: [...new Set([targetDir, resolve(join(repoRoot, "target"))])],
  exists: existsSync,
});
if (!pdfium.path) {
  missing.push(`  ${pdfiumLib} -- looked in:\n${pdfium.tried.map((t) => `      ${t}`).join("\n")}\n    run: cargo xtask fetch-pdfium   (or point LEXMECHANIC_PDFIUM at the folder holding it)`);
}

// Non-system DLLs the exes import (on Windows: DirectML.dll, pulled in by
// `ort`). They are loaded before main(), so one missing makes the server
// exit immediately.
let dlls = [];
if (platform === "win32" && missing.length === 0) {
  const imports = readExeImports({ [serverExe]: join(releaseDir, serverExe), [workerExe]: join(releaseDir, workerExe) }, readFileSync);
  const systemDir = process.env.SystemRoot ? join(process.env.SystemRoot, "System32") : null;
  const plan = planDllBundle({ imports, releaseDir, systemDir, exists: existsSync, realpath: realpathSync });
  for (const u of plan.unresolved) {
    missing.push(`  ${u.dll} (imported by ${u.neededBy.join(", ")}) is neither beside the built exes in ${releaseDir}\n    nor a Windows system DLL, or it is a link that points nowhere. Rebuild with: cargo build --release --workspace`);
  }
  dlls = plan.bundle;
}

if (missing.length > 0) {
  console.error("Cannot package: the app bundles these, and they are not available yet.\n" + missing.join("\n"));
  process.exit(1);
}

// Destinations match what `server.rs` probes: the binary at the resource
// root, `web-dist/` beside it.
//
// The files are staged into their own folder first. `tauri-build` copies
// each resource into cargo's `release/` directory (next to the app exe);
// the exes already live there, so copying them from there is a file
// copied onto itself, which Windows refuses with "used by another
// process". A staging folder also turns the DirectML link into a plain
// file and puts the exact set of shipped files in one place to look at.
const stage = join(targetDir, "bundle-stage");
rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });
const staged = (src, name) => {
  copyFileSync(src, join(stage, name));
  return join(stage, name);
};
for (const exe of [serverExe, workerExe]) staged(join(releaseDir, exe), exe);
const stagedPdfium = staged(pdfium.path, pdfiumLib);
const stagedDlls = dlls.map((d) => ({ name: d.name, source: staged(d.source, d.name) }));
const bundle = {
  resources: bundleResources(platform, { releaseDir: stage, pdfium: stagedPdfium, dlls: stagedDlls }),
};
if (platform === "win32") {
  // One installer, per user: no admin prompt, and the Start menu shortcut
  // and uninstaller are NSIS defaults. Windows-only so the same script
  // still works on a machine where NSIS is not a target.
  bundle.targets = ["nsis"];
}

const out = join(srcTauri, "tauri.bundle.conf.json");
writeFileSync(out, JSON.stringify({ bundle }, null, 2) + "\n");
console.log(`wrote ${out}`);
console.log(`  server: ${serverExe}   pdfium: ${pdfium.path}`);
for (const d of dlls) console.log(`  bundling ${d.name} from ${d.source}`);
