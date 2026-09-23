// Writes the bundle-only Tauri config used by `npm run tauri:build`.
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
// Node already knows the platform, so it writes the right name.

import { existsSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const srcTauri = join(here, "..", "src-tauri");
const repoRoot = join(here, "..", "..");

const serverExe = process.platform === "win32" ? "calibre_srv.exe" : "calibre_srv";

// Paths inside the config are relative to `src-tauri/`, which is where
// Tauri resolves them from.
const serverSource = `../../target/release/${serverExe}`;
const webDistSource = "../../web/dist/";

// Fail here, with a sentence that says what to run, rather than letting
// Tauri fail later with a bare "resource path doesn't exist".
const missing = [];
if (!existsSync(join(repoRoot, "target", "release", serverExe))) {
  missing.push(`  ${serverExe} — run: cargo build --release --workspace`);
}
if (!existsSync(join(repoRoot, "web", "dist", "index.html"))) {
  missing.push("  web/dist — run: npm install && npm run build   (in web/)");
}
if (missing.length > 0) {
  console.error("Cannot package: the app bundles these, and they are not built yet.\n" + missing.join("\n"));
  process.exit(1);
}

// Destinations match what `server.rs` probes: the binary at the resource
// root, `web-dist/` beside it.
const config = {
  bundle: {
    resources: {
      [serverSource]: serverExe,
      [webDistSource]: "web-dist/",
    },
  },
};

const out = join(srcTauri, "tauri.bundle.conf.json");
writeFileSync(out, JSON.stringify(config, null, 2) + "\n");
console.log(`wrote ${out} (server: ${serverExe})`);
