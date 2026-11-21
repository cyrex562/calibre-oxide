import assert from "node:assert/strict";
import { test } from "node:test";
import { bundleResources, findPdfium, planDllBundle } from "./bundle-resources.mjs";
import { readPeImports } from "./pe-imports.mjs";

const rel = "C:/t/release";

test("the bundle ships the PDF worker, PDFium and web UI next to the server, flat", () => {
  for (const platform of ["win32", "linux", "darwin"]) {
    const res = bundleResources(platform, { releaseDir: rel, pdfium: "C:/t/pdfium/bin/x" });
    const dests = Object.values(res);
    const sfx = platform === "win32" ? ".exe" : "";
    assert.ok(dests.includes(`calibre_srv${sfx}`));
    assert.ok(dests.includes(`calibre_pdf_worker${sfx}`));
    assert.ok(dests.includes("web-dist/"));
    // Flat: the worker and PDFium must be beside the server.
    for (const d of dests.filter((d) => !d.endsWith("/"))) assert.ok(!d.includes("/"));
    assert.equal(dests.length, 4);
  }
});

test("a bundle cannot be described without PDFium", () => {
  assert.throws(() => bundleResources("win32", { releaseDir: rel, pdfium: null }), /pdfium\.dll was not found/);
});

test("extra DLLs ship flat, from their real (link-resolved) file", () => {
  const res = bundleResources("win32", { releaseDir: rel, pdfium: "C:/p/pdfium.dll", dlls: [{ name: "DirectML.dll", source: "C:\\Users\\x\\ort\\DirectML.dll" }] });
  assert.equal(res["C:/Users/x/ort/DirectML.dll"], "DirectML.dll");
});

// ---- pdfium search order -------------------------------------------------

const fsWith = (...present) => (p) => present.includes(p);
const targets = ["C:/cargo-target"];

test("PDFium: the environment variable wins over everything else", () => {
  const r = findPdfium({
    platform: "win32",
    envDir: "C:\\env\\bin",
    targetDirs: targets,
    exists: fsWith("C:/env/bin/pdfium.dll", "C:/cargo-target/pdfium/bin/pdfium.dll", "C:/cargo-target/release/pdfium.dll"),
  });
  assert.equal(r.path, "C:/env/bin/pdfium.dll");
});

test("PDFium: the variable may name the file itself", () => {
  const r = findPdfium({ platform: "win32", envDir: "D:/lib/pdfium.dll", targetDirs: targets, exists: fsWith("D:/lib/pdfium.dll") });
  assert.equal(r.path, "D:/lib/pdfium.dll");
});

test("PDFium: then target/pdfium/bin, then target/release", () => {
  const both = findPdfium({ platform: "win32", targetDirs: targets, exists: fsWith("C:/cargo-target/pdfium/bin/pdfium.dll", "C:/cargo-target/release/pdfium.dll") });
  assert.equal(both.path, "C:/cargo-target/pdfium/bin/pdfium.dll");
  const onlyRelease = findPdfium({ platform: "win32", targetDirs: targets, exists: fsWith("C:/cargo-target/release/pdfium.dll") });
  assert.equal(onlyRelease.path, "C:/cargo-target/release/pdfium.dll");
});

test("PDFium: an unset or blank variable is skipped, and a variable pointing at nothing falls through", () => {
  assert.equal(findPdfium({ platform: "win32", envDir: "  ", targetDirs: targets, exists: fsWith("C:/cargo-target/release/pdfium.dll") }).path, "C:/cargo-target/release/pdfium.dll");
  assert.equal(findPdfium({ platform: "win32", envDir: "C:/gone", targetDirs: targets, exists: fsWith("C:/cargo-target/release/pdfium.dll") }).path, "C:/cargo-target/release/pdfium.dll");
});

test("PDFium: missing everywhere is reported with every place that was tried", () => {
  const r = findPdfium({ platform: "win32", envDir: "C:/env", targetDirs: targets, exists: () => false });
  assert.equal(r.path, null);
  assert.deepEqual(r.tried, ["C:/env/pdfium.dll", "C:/cargo-target/pdfium/bin/pdfium.dll", "C:/cargo-target/release/pdfium.dll"]);
});

test("PDFium: other platforms look for their own library name in lib/", () => {
  const r = findPdfium({ platform: "linux", targetDirs: targets, exists: fsWith("C:/cargo-target/pdfium/lib/libpdfium.so") });
  assert.equal(r.path, "C:/cargo-target/pdfium/lib/libpdfium.so");
});

// ---- DLL bundling ----------------------------------------------------------

const sys = "C:/Windows/System32";
const imports = {
  "calibre_srv.exe": ["KERNEL32.dll", "DirectML.dll", "api-ms-win-crt-string-l1-1-0.dll", "VCRUNTIME140.dll", "dxgi.dll"],
  "calibre_pdf_worker.exe": ["kernel32.dll", "directml.DLL", "MSVCP140_1.dll"],
};
const ortCache = "C:/Users/x/AppData/Local/ort.pyke.io/DirectML.dll";

test("DirectML.dll is bundled from the link target, even though Windows has an older copy", () => {
  const plan = planDllBundle({
    imports,
    releaseDir: rel,
    systemDir: sys,
    // The release dir holds a link; System32 also has (an older) DirectML.dll.
    exists: fsWith(`${rel}/DirectML.dll`, `${sys}/DirectML.dll`, `${sys}/KERNEL32.dll`, `${sys}/dxgi.dll`),
    realpath: (p) => (p === `${rel}/DirectML.dll` ? ortCache.replace(/\//g, "\\") : p),
  });
  assert.deepEqual(plan.unresolved, []);
  // Once, although both exes import it with different capitalisation.
  assert.deepEqual(plan.bundle, [{ name: "DirectML.dll", source: ortCache }]);
});

test("system DLLs, API sets and the VC runtime are not bundled", () => {
  const plan = planDllBundle({ imports, releaseDir: rel, systemDir: sys, exists: fsWith(`${sys}/KERNEL32.dll`, `${sys}/dxgi.dll`, `${sys}/DirectML.dll`), realpath: (p) => p });
  assert.deepEqual(plan.bundle, []);
  assert.deepEqual(plan.unresolved, []);
});

test("a dangling DirectML link is a failure that names the DLL and who needs it", () => {
  const plan = planDllBundle({
    imports,
    releaseDir: rel,
    systemDir: sys,
    exists: fsWith(`${rel}/DirectML.dll`, `${sys}/KERNEL32.dll`, `${sys}/dxgi.dll`),
    realpath: () => {
      throw new Error("ENOENT");
    },
  });
  assert.equal(plan.unresolved.length, 1);
  assert.equal(plan.unresolved[0].dll, "DirectML.dll");
  assert.deepEqual(plan.unresolved[0].neededBy.sort(), ["calibre_pdf_worker.exe", "calibre_srv.exe"]);
});

test("an import found nowhere is unresolved", () => {
  const plan = planDllBundle({ imports: { "a.exe": ["mystery.dll"] }, releaseDir: rel, systemDir: sys, exists: () => false, realpath: (p) => p });
  assert.deepEqual(plan.unresolved, [{ dll: "mystery.dll", neededBy: ["a.exe"] }]);
});

// ---- PE reader -------------------------------------------------------------

/** Builds the smallest PE32+ with one section holding an import table naming `dlls`. */
function tinyPe(dlls) {
  const buf = Buffer.alloc(0x400);
  buf.writeUInt16LE(0x5a4d, 0);
  buf.writeUInt32LE(0x80, 0x3c);
  buf.write("PE\0\0", 0x80, "latin1");
  buf.writeUInt16LE(1, 0x80 + 6); // one section
  buf.writeUInt16LE(240, 0x80 + 20); // optional header size
  const opt = 0x80 + 24;
  buf.writeUInt16LE(0x20b, opt);
  buf.writeUInt32LE(16, opt + 108); // number of data directories
  const sectionVa = 0x1000;
  const sectionRaw = 0x200;
  const tableOff = sectionRaw; // descriptors first, then names
  const descriptors = (dlls.length + 1) * 20;
  buf.writeUInt32LE(sectionVa, opt + 112 + 8); // directory 1: import table RVA
  buf.writeUInt32LE(descriptors, opt + 112 + 12);
  let nameOff = tableOff + descriptors;
  dlls.forEach((d, i) => {
    buf.writeUInt32LE(sectionVa + (nameOff - sectionRaw), tableOff + i * 20 + 12);
    buf.write(d + "\0", nameOff, "latin1");
    nameOff += d.length + 1;
  });
  const sec = opt + 240;
  buf.writeUInt32LE(0x200, sec + 8); // virtual size
  buf.writeUInt32LE(sectionVa, sec + 12);
  buf.writeUInt32LE(0x200, sec + 16); // raw size
  buf.writeUInt32LE(sectionRaw, sec + 20);
  return buf;
}

test("the PE reader lists the DLLs an executable imports", () => {
  assert.deepEqual(readPeImports(tinyPe(["KERNEL32.dll", "DirectML.dll"])).imports, ["KERNEL32.dll", "DirectML.dll"]);
});

test("the PE reader rejects a file that is not a PE", () => {
  assert.throws(() => readPeImports(Buffer.from("#!/bin/sh\n".padEnd(100))), /not a PE file/);
});
