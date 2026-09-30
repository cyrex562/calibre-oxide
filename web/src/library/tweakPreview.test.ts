import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

import { buildPreview, previewableFiles, readPreviewSources, resolveName, rewriteCssUrls } from "./tweakPreview";

/**
 * Serves a fake session over `fetch`, so a test can say exactly what the
 * book contains. `/tweak/file/...` returns text and `/tweak/raw/...`
 * returns bytes, matching the two real routes.
 */
function stubSession(files: Record<string, string>) {
  const fetchMock = vi.fn(async (url: string) => {
    const match = /^\/tweak\/(file|raw)\/[^/]+\/(.*)$/.exec(url);
    if (!match) return { ok: false, status: 404 };
    const name = decodeURIComponent(match[2].split("/").map(decodeURIComponent).join("/"));
    const body = files[name];
    if (body === undefined) return { ok: false, status: 404 };
    return { ok: true, status: 200, text: async () => body, blob: async () => new Blob([body]) };
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

beforeEach(() => {
  let counter = 0;
  // jsdom implements neither, and the module must revoke what it creates.
  vi.stubGlobal("URL", Object.assign(Object.create(URL), URL, {
    createObjectURL: vi.fn(() => `blob:preview/${++counter}`),
    revokeObjectURL: vi.fn(),
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("resolveName", () => {
  it("resolves a sibling reference against the referring file's folder", () => {
    expect(resolveName("OEBPS/text/ch1.xhtml", "style.css")).toBe("OEBPS/text/style.css");
  });

  it("collapses .. segments", () => {
    expect(resolveName("OEBPS/text/ch1.xhtml", "../styles/main.css")).toBe("OEBPS/styles/main.css");
  });

  it("refuses a reference that climbs above the container root", () => {
    // Otherwise a malformed book could make the preview request something
    // outside the container.
    expect(resolveName("OEBPS/ch1.xhtml", "../../etc/passwd")).toBeNull();
  });

  it("leaves absolute and protocol-relative URLs to the browser", () => {
    expect(resolveName("a/b.xhtml", "https://example.com/x.css")).toBeNull();
    expect(resolveName("a/b.xhtml", "//example.com/x.css")).toBeNull();
    expect(resolveName("a/b.xhtml", "data:text/css,body{}")).toBeNull();
  });

  it("drops fragments and queries", () => {
    expect(resolveName("a/b.xhtml", "c.xhtml#part2")).toBe("a/c.xhtml");
  });
});

describe("readPreviewSources", () => {
  const opf = `<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><manifest>
      <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
      <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
      <item id="css" href="styles/main.css" media-type="text/css"/>
    </manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>`;

  it("returns content documents in spine order, not manifest order", async () => {
    stubSession({ "OEBPS/content.opf": opf });
    const sources = await readPreviewSources("s1", ["OEBPS/content.opf", "OEBPS/text/ch1.xhtml"]);
    expect(sources.spine).toEqual(["OEBPS/text/ch1.xhtml", "OEBPS/text/ch2.xhtml"]);
    expect(sources.opfName).toBe("OEBPS/content.opf");
  });

  it("falls back to content-looking files when the OPF cannot be parsed", async () => {
    stubSession({ "OEBPS/content.opf": "this is not xml at all <<<" });
    const files = ["OEBPS/content.opf", "OEBPS/ch1.xhtml", "OEBPS/styles/main.css", "OEBPS/cover.jpg"];
    const sources = await readPreviewSources("s1", files);
    // A book being edited is often mid-breakage; the preview still has to
    // offer something.
    expect(previewableFiles(sources, files)).toEqual(["OEBPS/ch1.xhtml"]);
  });

  it("copes with a book that has no OPF at all", async () => {
    stubSession({});
    const files = ["ch1.html"];
    const sources = await readPreviewSources("s1", files);
    expect(sources.opfName).toBeNull();
    expect(previewableFiles(sources, files)).toEqual(["ch1.html"]);
  });
});

describe("buildPreview", () => {
  const content = `<html><head><link rel="stylesheet" href="../styles/main.css"/></head>
    <body><p>Hello</p><img src="../images/fig.png"/><a href="ch2.xhtml">next</a></body></html>`;

  it("inlines the stylesheet so an edit is not served from cache", async () => {
    stubSession({
      "OEBPS/text/ch1.xhtml": content,
      "OEBPS/styles/main.css": "p { color: rebeccapurple }",
      "OEBPS/images/fig.png": "PNGDATA",
    });
    const { html } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    expect(html).toContain("rebeccapurple");
    expect(html).toContain('data-preview-from="OEBPS/styles/main.css"');
    // The <link> itself is gone -- a cached one is exactly what would
    // stop an edit from showing up.
    expect(html).not.toContain("<link");
  });

  /// The point of the whole feature: the unsaved buffer wins.
  it("prefers the editor's unsaved buffer over the session's saved copy", async () => {
    stubSession({
      "OEBPS/text/ch1.xhtml": content,
      "OEBPS/styles/main.css": "p { color: saved }",
      "OEBPS/images/fig.png": "PNGDATA",
    });
    const { html } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", { "OEBPS/styles/main.css": "p { color: unsaved }" });
    expect(html).toContain("color: unsaved");
    expect(html).not.toContain("color: saved");
  });

  it("rewrites image references to blob URLs", async () => {
    stubSession({
      "OEBPS/text/ch1.xhtml": content,
      "OEBPS/styles/main.css": "p{}",
      "OEBPS/images/fig.png": "PNGDATA",
    });
    const { html, blobUrls } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    expect(html).toContain('src="blob:preview/');
    expect(blobUrls.length).toBeGreaterThan(0);
  });

  it("makes links inert so the preview cannot navigate away", async () => {
    stubSession({ "OEBPS/text/ch1.xhtml": content, "OEBPS/styles/main.css": "p{}", "OEBPS/images/fig.png": "X" });
    const { html } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    expect(html).toContain("data-preview-inert");
    expect(html).not.toContain('href="ch2.xhtml"');
  });

  it("still renders the page when a referenced file is missing", async () => {
    // Normal mid-edit: a stylesheet renamed but not yet re-linked.
    stubSession({ "OEBPS/text/ch1.xhtml": content });
    const { html } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    expect(html).toContain("Hello");
  });

  it("reports every blob URL it created so the caller can revoke them", async () => {
    stubSession({
      "OEBPS/text/ch1.xhtml": `<html><body><img src="a.png"/><img src="b.png"/></body></html>`,
      "OEBPS/text/a.png": "A",
      "OEBPS/text/b.png": "B",
    });
    const { blobUrls } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    // Two distinct images, two blobs -- a leak here is one per keystroke.
    expect(blobUrls).toHaveLength(2);
  });

  it("fetches a resource used twice only once", async () => {
    const fetchMock = stubSession({
      "OEBPS/text/ch1.xhtml": `<html><body><img src="same.png"/><img src="same.png"/></body></html>`,
      "OEBPS/text/same.png": "A",
    });
    const { blobUrls } = await buildPreview("s1", "OEBPS/text/ch1.xhtml", {});
    expect(blobUrls).toHaveLength(1);
    const pngFetches = fetchMock.mock.calls.filter(([url]) => String(url).includes("same.png"));
    expect(pngFetches).toHaveLength(1);
  });
});

describe("rewriteCssUrls", () => {
  it("resolves url() against the stylesheet's own folder, not the page's", async () => {
    // This is why stylesheets are inlined per-file: a shared stylesheet in
    // styles/ refers to fonts/ beside itself, not beside the chapter.
    const seen: string[] = [];
    const css = await rewriteCssUrls("@font-face { src: url('../fonts/x.otf') }", "OEBPS/styles/main.css", async (name) => {
      seen.push(name);
      return "blob:font";
    });
    expect(seen).toEqual(["OEBPS/fonts/x.otf"]);
    expect(css).toContain('url("blob:font")');
  });

  it("leaves a url() it cannot resolve exactly as it was", async () => {
    const css = await rewriteCssUrls("a { background: url(https://example.com/x.png) }", "s/main.css", async () => "blob:never");
    expect(css).toContain("https://example.com/x.png");
  });
});
