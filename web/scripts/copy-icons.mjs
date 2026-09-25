// Copies calibre's own toolbar icons into `public/icons/`.
//
// The project is GPL-3 and a derivative of calibre, and calibre's icon
// set is already vendored at `old_src/resources/images/` (192 PNGs,
// GPL-3, see NOTICE). Using them is both permitted and the point: an
// icon toolbar that looks like calibre's is the strongest single cue
// that this *is* calibre, and drawing a replacement set would be a lot
// of work to arrive somewhere worse.
//
// Copied rather than committed a second time, for the same reason
// `copy-pdfjs-assets.mjs` copies: the files already exist in the repo,
// and a second copy in `web/public/` would be one more thing to keep
// in step by hand.
//
// Only the curated subset below is copied. `old_src/resources/images/`
// holds 192 files, most of which are for parts of calibre this port
// does not have -- shipping all of them would put ~1.5MB of unused
// images into every build.
//
// See docs/UI_DESIGN.md §2.4 for the action-to-icon mapping.

import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "..", "..", "old_src", "resources", "images");
const dest = join(here, "..", "public", "icons");

/**
 * The icons the UI actually references.
 *
 * Grouped the way the toolbar is, so adding a button and forgetting
 * its icon is visible here rather than as a broken image at runtime.
 */
const ICONS = [
  // Toolbar: getting books in
  "add_book", "tb_folder", "news",
  // Toolbar: metadata
  "edit_input", "download-metadata", "default_cover", "merge_books", "tags", "template_funcs",
  // Toolbar: conversion and editing
  "convert", "polish", "tweak", "unpack-book", "sync",
  // Toolbar: reading
  "view", "external-link", "quickview", "similar",
  // Toolbar: getting books out
  "save", "mail", "catalog", "bookshelf",
  // Toolbar: removal
  "remove_books", "edit-undo",
  // Toolbar: right-hand cluster
  "lt", "config", "help",
  // Search bar
  "search", "gear", "fts", "folder_saved_search", "sort", "minus", "arrow-down",
  "dialog_error", "window-close", "vl",
  // View switching and layout
  "grid", "layout", "h-ellipsis",
  // Library tools
  "reports", "merge", "column", "highlight", "random", "jobs", "marked",
  // Category browser
  "user_profile", "series", "publisher", "languages", "rating",
];

if (!existsSync(src)) {
  // Not fatal: someone may be building from a source tree without the
  // upstream reference checked out. The UI falls back to text labels.
  console.warn(`copy-icons: ${src} not found — skipping (the toolbar will render text-only)`);
  process.exit(0);
}

// Cleared first so an icon removed from the list above does not linger
// in `public/` and quietly keep working.
rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });

const missing = [];
let copied = 0;
for (const name of ICONS) {
  const from = join(src, `${name}.png`);
  if (!existsSync(from)) {
    missing.push(name);
    continue;
  }
  copyFileSync(from, join(dest, `${name}.png`));
  copied++;
}

// A few icons ship a purpose-drawn dark variant rather than relying on
// the CSS filter. Copy any that exist for the icons we use.
for (const name of ICONS) {
  const variant = `${name}-for-dark-theme.png`;
  if (existsSync(join(src, variant))) {
    copyFileSync(join(src, variant), join(dest, variant));
    copied++;
  }
}

if (missing.length > 0) {
  // Loud but not fatal — a missing icon is a text label, not a crash,
  // and failing the build over one would be disproportionate.
  console.warn(`copy-icons: ${missing.length} not found in ${src}: ${missing.join(", ")}`);
}
console.log(`copy-icons: ${copied} icons -> ${dest}`);

// Sanity check that the source really is calibre's icon directory and
// not something else with the right path, which would otherwise show
// up as a silently empty toolbar.
if (copied === 0) {
  const sample = readdirSync(src).slice(0, 5).join(", ");
  console.warn(`copy-icons: copied nothing. ${src} contains: ${sample}…`);
}
