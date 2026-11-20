// Pure description of what the installer ships, split out of
// `bundle-config.mjs` so `bundle-resources.test.mjs` can check it without
// touching the disk or Tauri.
//
// Everything lands flat in the resource directory, next to the server:
// `calibre_srv` looks for `calibre_pdf_worker` (the out-of-process PDF
// worker, `calibre_db::pdf_worker::locate_worker`) and for `pdfium.dll`
// (`rasterize::candidate_library_paths`) in its OWN directory. The app
// runs `calibre_srv` straight out of the resource directory
// (`server.rs::resolve_calibre_srv_binary`), so putting the worker and
// PDFium beside it is what makes both found. Without the worker the
// server silently falls back to slow in-process PDFium.

export function platformNames(platform) {
  const win = platform === "win32";
  return {
    serverExe: win ? "calibre_srv.exe" : "calibre_srv",
    workerExe: win ? "calibre_pdf_worker.exe" : "calibre_pdf_worker",
    pdfiumLib: win ? "pdfium.dll" : platform === "darwin" ? "libpdfium.dylib" : "libpdfium.so",
  };
}

/**
 * @param {string} platform  process.platform
 * @param {boolean} hasPdfium whether PDFium is present in target/release
 * @returns {Record<string, string>} source (relative to src-tauri) -> name in the bundle
 */
export function bundleResources(platform, hasPdfium) {
  const { serverExe, workerExe, pdfiumLib } = platformNames(platform);
  return {
    [`../../target/release/${serverExe}`]: serverExe,
    [`../../target/release/${workerExe}`]: workerExe,
    "../../web/dist/": "web-dist/",
    ...(hasPdfium ? { [`../../target/release/${pdfiumLib}`]: pdfiumLib } : {}),
  };
}
