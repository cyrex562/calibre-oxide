import assert from "node:assert/strict";
import { test } from "node:test";
import { bundleResources } from "./bundle-resources.mjs";

test("the bundle ships the PDF worker next to the server, flat", () => {
  for (const platform of ["win32", "linux", "darwin"]) {
    const res = bundleResources(platform, true);
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

test("PDFium is optional", () => {
  assert.equal(Object.values(bundleResources("win32", false)).length, 3);
});
