// Pure description of what the installer ships, split out of
// `bundle-config.mjs` so `bundle-resources.test.mjs` can check it without
// touching the real disk or Tauri. Everything that looks at the file
// system takes its lookups as arguments for the same reason.
//
// Everything lands flat in the resource directory, next to the server:
// `calibre_srv` looks for `calibre_pdf_worker` (the out-of-process PDF
// worker, `calibre_db::pdf_worker::locate_worker`) and for `pdfium.dll`
// (`rasterize::candidate_library_paths`) in its OWN directory. The app
// runs `calibre_srv` straight out of the resource directory
// (`server.rs::resolve_calibre_srv_binary`), so putting the worker,
// PDFium and any DLL the exes import beside it is what makes them found.
// Without the worker the server silently falls back to slow in-process
// PDFium.

import { readPeImports } from "./pe-imports.mjs";

export function platformNames(platform) {
  const win = platform === "win32";
  return {
    serverExe: win ? "calibre_srv.exe" : "calibre_srv",
    workerExe: win ? "calibre_pdf_worker.exe" : "calibre_pdf_worker",
    pdfiumLib: win ? "pdfium.dll" : platform === "darwin" ? "libpdfium.dylib" : "libpdfium.so",
  };
}

const slash = (p) => p.replace(/\\/g, "/");
const join = (...parts) => parts.map(slash).join("/").replace(/\/+/g, "/");

/**
 * Where PDFium may be, in the order it is looked for, and the first that
 * holds the library. `$CALIBRE_OXIDE_PDFIUM` first because it is what the
 * rest of the workspace reads (`rasterize::LIBRARY_PATH_VAR`) and so what
 * a developer has already set; it may name the directory or the file.
 *
 * @param {object} o
 * @param {string} o.platform
 * @param {string | undefined} o.envDir  value of CALIBRE_OXIDE_PDFIUM
 * @param {string[]} o.targetDirs        cargo target dirs, most specific first
 * @param {(p: string) => boolean} o.exists
 * @returns {{path: string | null, tried: string[]}}
 */
export function findPdfium({ platform, envDir, targetDirs, exists }) {
  const { pdfiumLib } = platformNames(platform);
  const candidates = [];
  if (envDir && envDir.trim()) {
    const e = slash(envDir.trim());
    candidates.push(e.toLowerCase().endsWith(pdfiumLib.toLowerCase()) ? e : join(e, pdfiumLib));
  }
  for (const t of targetDirs) {
    // `cargo xtask fetch-pdfium` stages into target/pdfium/{bin,lib}; a
    // developer who copied the library next to the binaries has it in release/.
    candidates.push(join(t, "pdfium", platform === "win32" ? "bin" : "lib", pdfiumLib));
    candidates.push(join(t, "release", pdfiumLib));
  }
  const tried = [...new Set(candidates)];
  return { path: tried.find((c) => exists(c)) ?? null, tried };
}

// DLLs a machine with the VC++ redistributable already has. Never bundled:
// Microsoft's licence terms for these want them installed, not copied, and
// the README lists the redistributable as a requirement.
const VC_RUNTIME = /^(vcruntime|msvcp|vcomp|concrt|vccorlib)\d*(_\d+)?\.dll$/i;
// The universal CRT and API sets are part of Windows itself.
const WINDOWS_API_SET = /^(api-ms-win-|ext-ms-win-)/i;

/**
 * Which DLLs the exes import that must travel with the installer.
 *
 * A DLL is bundled when it sits beside the built exes (that is where the
 * `ort` build puts `DirectML.dll`), even if Windows also has a copy: the
 * copy in System32 is an older DirectML than the one `ort` was built
 * against, and the app directory is searched first, so shipping ours is
 * what makes the version we tested the version that loads. Anything else
 * must be a Windows system DLL or the VC runtime, or the build fails here
 * rather than the installed app failing to start.
 *
 * @param {object} o
 * @param {Record<string, string[]>} o.imports  exe name -> imported DLL names
 * @param {string} o.releaseDir                 where the exes were built
 * @param {string | null} o.systemDir           e.g. C:/Windows/System32
 * @param {(p: string) => boolean} o.exists     true for a file or a working link
 * @param {(p: string) => string} o.realpath    resolves symlinks; throws if dangling
 * @returns {{bundle: {name: string, source: string}[], unresolved: {dll: string, neededBy: string[]}[]}}
 */
export function planDllBundle({ imports, releaseDir, systemDir, exists, realpath }) {
  const neededBy = new Map(); // lower-case name -> {name, exes}
  for (const [exe, dlls] of Object.entries(imports)) {
    for (const dll of dlls) {
      const key = dll.toLowerCase();
      if (!neededBy.has(key)) neededBy.set(key, { name: dll, exes: new Set() });
      neededBy.get(key).exes.add(exe);
    }
  }

  const bundle = [];
  const unresolved = [];
  for (const { name, exes } of [...neededBy.values()].sort((a, b) => a.name.localeCompare(b.name))) {
    if (WINDOWS_API_SET.test(name) || VC_RUNTIME.test(name)) continue;
    const beside = join(releaseDir, name);
    if (exists(beside)) {
      let source;
      try {
        source = slash(realpath(beside));
      } catch {
        unresolved.push({ dll: name, neededBy: [...exes] });
        continue;
      }
      bundle.push({ name, source });
      continue;
    }
    if (systemDir && exists(join(systemDir, name))) continue;
    unresolved.push({ dll: name, neededBy: [...exes] });
  }
  return { bundle, unresolved };
}

/** Reads the (hard) imports of the built exes, keyed by exe name. */
export function readExeImports(exePaths, readFile) {
  const out = {};
  for (const [name, path] of Object.entries(exePaths)) out[name] = readPeImports(readFile(path)).imports;
  return out;
}

/**
 * @param {string} platform  process.platform
 * @param {object} o
 * @param {string} o.releaseDir   cargo's release dir (absolute)
 * @param {string | null} o.pdfium  absolute path of the PDFium library to ship (required)
 * @param {{name: string, source: string}[]} [o.dlls]  extra DLLs, already resolved to real files
 * @returns {Record<string, string>} source (absolute or relative to src-tauri) -> name in the bundle
 */
export function bundleResources(platform, { releaseDir, pdfium, dlls = [] }) {
  const { serverExe, workerExe, pdfiumLib } = platformNames(platform);
  if (!pdfium) throw new Error(`${pdfiumLib} was not found; refusing to describe a bundle without it`);
  const res = {
    [join(releaseDir, serverExe)]: serverExe,
    [join(releaseDir, workerExe)]: workerExe,
    "../../web/dist/": "web-dist/",
    [slash(pdfium)]: pdfiumLib,
  };
  for (const d of dlls) res[slash(d.source)] = d.name;
  return res;
}
