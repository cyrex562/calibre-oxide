// Copies PDF.js's runtime data files into `public/` so the build
// emits them alongside the app (issue 2.1 of the #816 epic).
//
// PDF.js needs two directories of data at *runtime*, not build time:
//
//   standard_fonts/  the 14 standard PDF fonts (Helvetica, Times,
//                    Courier, Symbol, ZapfDingbats). A PDF that names
//                    one carries no glyphs for it, so without these
//                    PDF.js warns "Ensure that the standardFontDataUrl
//                    API parameter is provided" and the text does not
//                    render properly. Our own `ebook_convert` output
//                    uses these fonts, so this is not an edge case.
//
//   cmaps/           character maps for CJK encodings.
//
// They are copied rather than committed: they are ~2.5MB of binary
// data that already exists in `node_modules`, and vendoring it into
// git would mean updating it by hand whenever pdfjs-dist moves.
//
// They are *data files served on demand*, not bundle content -- a
// reader only fetches the specific font or cmap a given PDF asks for.

import { cp, mkdir, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const from = join(root, "node_modules", "pdfjs-dist");
const to = join(root, "public", "pdfjs");

if (!existsSync(from)) {
  console.error("pdfjs-dist is not installed -- run `npm install` first");
  process.exit(1);
}

await rm(to, { recursive: true, force: true });
await mkdir(to, { recursive: true });

for (const dir of ["standard_fonts", "cmaps"]) {
  const src = join(from, dir);
  if (!existsSync(src)) {
    console.error(`pdfjs-dist is missing ${dir}/ -- PDFs using it will not render correctly`);
    process.exit(1);
  }
  await cp(src, join(to, dir), { recursive: true });
}

console.log("copied PDF.js standard_fonts and cmaps into public/pdfjs/");
