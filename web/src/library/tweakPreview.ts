// The Tweak Book editor's live preview (#960).
//
// # Why this does not reuse the reader
//
// `reader/unserialize.ts` already renders book content into a sandboxed
// iframe, and reusing it was the obvious first idea. It does not fit: it
// fetches server-rendered *reader-json*, keyed by the committed book's
// size and mtime. A tweak session holds edits that have not been
// committed, so there is nothing for it to fetch -- and previewing a
// re-serialisation would mean a bug in that step looked like a bug in the
// user's own CSS.
//
// So this assembles the preview from the session's own bytes: the content
// file as it is *in the editor right now*, with every resource it
// references resolved to a blob URL fetched from the session.
//
// # Why the editor buffer matters
//
// The preview re-renders while the user types, before anything is saved.
// It therefore prefers the live editor buffer over the session's saved
// copy for the file being edited -- otherwise a "live preview" shows the
// last saved state, which is the one thing the user can already see.

/** A file's current text as the editor has it, keyed by session name. */
export type Overrides = Record<string, string>;

export interface PreviewSources {
  /** Content documents in spine order. Empty if the OPF could not be read. */
  spine: string[];
  /** The OPF's own name, for resolving hrefs relative to it. */
  opfName: string | null;
}

function sessionUrl(kind: "file" | "raw", sessionId: string, name: string): string {
  const path = name.split("/").map(encodeURIComponent).join("/");
  return `/tweak/${kind}/${encodeURIComponent(sessionId)}/${path}`;
}

/** `GET /tweak/file/…` -- text, for the OPF and stylesheets. */
async function fetchText(sessionId: string, name: string): Promise<string> {
  const res = await fetch(sessionUrl("file", sessionId, name));
  if (!res.ok) throw new Error(`${name}: ${res.status}`);
  return await res.text();
}

/**
 * `GET /tweak/raw/…` -- bytes, for images and fonts.
 *
 * `/tweak/file` refuses anything that is not editable text, which is
 * right for an editor buffer and useless for a preview: the files it
 * refuses are exactly the ones a page needs to look like itself.
 */
async function fetchBlob(sessionId: string, name: string): Promise<Blob> {
  const res = await fetch(sessionUrl("raw", sessionId, name));
  if (!res.ok) throw new Error(`${name}: ${res.status}`);
  return await res.blob();
}

/**
 * Resolves `href` against the directory of `base`, both `/`-separated
 * container names.
 *
 * Container names are not URLs, so `new URL()` is no help -- they have no
 * scheme, and `..` has to be collapsed by hand. A reference that climbs
 * above the container root returns `null` rather than being clamped to
 * it, so a malformed or hostile book cannot make the preview ask for
 * something outside the container.
 */
export function resolveName(base: string, href: string): string | null {
  const clean = href.split("#")[0].split("?")[0].trim();
  // An absolute URL or protocol-relative reference belongs to the
  // network, not the book; left alone for the browser to refuse.
  if (!clean || /^[a-z][a-z0-9+.-]*:/i.test(clean) || clean.startsWith("//")) return null;
  const parts = clean.startsWith("/") ? [] : base.split("/").slice(0, -1);
  for (const segment of clean.replace(/^\//, "").split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (parts.length === 0) return null;
      parts.pop();
      continue;
    }
    parts.push(segment);
  }
  return parts.length ? parts.join("/") : null;
}

/**
 * The book's content documents in spine order, read from the OPF in the
 * session.
 *
 * `POST /tweak/open` reports a flat file list with no ordering, so the
 * spine has to come from the OPF itself.
 */
export async function readPreviewSources(sessionId: string, files: string[]): Promise<PreviewSources> {
  const opfName = files.find((f) => f.toLowerCase().endsWith(".opf")) ?? null;
  if (!opfName) return { spine: [], opfName: null };

  let doc: Document;
  try {
    doc = new DOMParser().parseFromString(await fetchText(sessionId, opfName), "application/xml");
  } catch {
    return { spine: [], opfName };
  }
  if (doc.querySelector("parsererror")) return { spine: [], opfName };

  const hrefById = new Map<string, string>();
  for (const item of Array.from(doc.getElementsByTagName("item"))) {
    const id = item.getAttribute("id");
    const href = item.getAttribute("href");
    if (id && href) hrefById.set(id, href);
  }

  const spine: string[] = [];
  for (const ref of Array.from(doc.getElementsByTagName("itemref"))) {
    const href = hrefById.get(ref.getAttribute("idref") ?? "");
    const name = href ? resolveName(opfName, href) : null;
    if (name) spine.push(name);
  }
  return { spine, opfName };
}

/**
 * Content documents worth offering as a preview target: spine order when
 * the OPF could be read, and anything that looks like a content document
 * otherwise, so a malformed book still previews.
 */
export function previewableFiles(sources: PreviewSources, files: string[]): string[] {
  if (sources.spine.length > 0) return sources.spine;
  return files.filter((f) => /\.x?html?$/i.test(f));
}

/**
 * Rewrites `url(...)` inside a stylesheet to blob URLs, resolved against
 * `cssName`'s own directory -- which is why stylesheets are inlined
 * per-file below rather than concatenated.
 */
export async function rewriteCssUrls(css: string, cssName: string, blobUrlFor: (name: string) => Promise<string | null>): Promise<string> {
  const pattern = /url\(\s*(['"]?)([^'")]+)\1\s*\)/gi;
  const replacements = new Map<string, string>();

  for (const match of Array.from(css.matchAll(pattern))) {
    const raw = match[2].trim();
    const name = resolveName(cssName, raw);
    if (!name || replacements.has(raw)) continue;
    const url = await blobUrlFor(name);
    if (url) replacements.set(raw, url);
  }

  return css.replace(pattern, (whole, _quote, raw) => {
    const url = replacements.get(String(raw).trim());
    return url ? `url("${url}")` : whole;
  });
}

/**
 * Builds a standalone HTML document for `contentName`, with every
 * stylesheet inlined and every other resource rewritten to a blob URL.
 *
 * Stylesheets are inlined rather than blob-linked for two reasons: an
 * edit shows up without the browser reusing a cached `<link>`, and each
 * one's `url()` references resolve against its own location rather than
 * the content file's.
 *
 * Returns the HTML and the blob URLs it created. The caller must revoke
 * them once the iframe has loaded -- otherwise every keystroke leaks one.
 */
export async function buildPreview(sessionId: string, contentName: string, overrides: Overrides): Promise<{ html: string; blobUrls: string[] }> {
  const blobUrls: string[] = [];
  const source = overrides[contentName] ?? (await fetchText(sessionId, contentName));
  const doc = new DOMParser().parseFromString(source, "text/html");

  // Cached per name, so a stylesheet behind two <link>s or an image used
  // twice costs one fetch and one blob.
  const resolved = new Map<string, Promise<string | null>>();
  const blobUrlFor = (name: string): Promise<string | null> => {
    const existing = resolved.get(name);
    if (existing) return existing;
    const pending = fetchBlob(sessionId, name)
      .then((blob) => {
        const url = URL.createObjectURL(blob);
        blobUrls.push(url);
        return url;
      })
      // A missing resource is normal mid-edit -- a stylesheet renamed but
      // not yet re-linked -- so the preview shows the rest of the page
      // rather than failing entirely.
      .catch(() => null);
    resolved.set(name, pending);
    return pending;
  };

  for (const link of Array.from(doc.querySelectorAll("link[href]"))) {
    const rel = (link.getAttribute("rel") ?? "").toLowerCase();
    if (!rel.split(/\s+/).includes("stylesheet")) continue;
    const name = resolveName(contentName, link.getAttribute("href") ?? "");
    if (!name) continue;
    let css: string;
    try {
      css = overrides[name] ?? (await fetchText(sessionId, name));
    } catch {
      continue;
    }
    const style = doc.createElement("style");
    style.textContent = await rewriteCssUrls(css, name, blobUrlFor);
    style.setAttribute("data-preview-from", name);
    link.replaceWith(style);
  }

  // Inline <style> blocks resolve their url()s against the content file.
  for (const style of Array.from(doc.querySelectorAll("style"))) {
    if (style.hasAttribute("data-preview-from")) continue;
    style.textContent = await rewriteCssUrls(style.textContent ?? "", contentName, blobUrlFor);
  }

  for (const [selector, attr] of [
    ["img[src]", "src"],
    ["source[src]", "src"],
    ["video[poster]", "poster"],
  ] as const) {
    for (const el of Array.from(doc.querySelectorAll(selector))) {
      const name = resolveName(contentName, el.getAttribute(attr) ?? "");
      if (!name) continue;
      const url = await blobUrlFor(name);
      if (url) el.setAttribute(attr, url);
    }
  }

  // SVG <image xlink:href>, which EPUB covers commonly use.
  for (const el of Array.from(doc.getElementsByTagName("image"))) {
    const raw = el.getAttributeNS("http://www.w3.org/1999/xlink", "href") ?? el.getAttribute("href") ?? "";
    const name = resolveName(contentName, raw);
    if (!name) continue;
    const url = await blobUrlFor(name);
    if (!url) continue;
    if (el.hasAttributeNS("http://www.w3.org/1999/xlink", "href")) el.setAttributeNS("http://www.w3.org/1999/xlink", "href", url);
    else el.setAttribute("href", url);
  }

  // A preview must not navigate itself away from the book.
  for (const anchor of Array.from(doc.querySelectorAll("a[href]"))) {
    anchor.setAttribute("href", "#");
    anchor.setAttribute("data-preview-inert", "");
  }

  return { html: `<!doctype html>${doc.documentElement.outerHTML}`, blobUrls };
}
