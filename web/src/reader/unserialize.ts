// Port of resources.pyj's `unserialize_html` + `load_resources`/
// `finalize_resources` -- turns the reader_json tree
// (calibre_ebooks::reader_json::serialize_document's real output
// shape) into a live DOM inside the reader iframe's own document, and
// resolves every virtualized resource reference (img/link src/href,
// inline style url()s) into a real fetched blob URL.
//
// Narrowed vs. upstream: this crate's own reader_json.rs never emits
// a 3-element (namespaced) attribute tuple -- `Dom` collapses foreign
// attribute prefixes into their bare local name during parsing (see
// render_book.rs's own module doc) -- so unlike the old RapydScript
// client, there's no `ns_map`/`setAttributeNS` branch to replicate.

import { fetchBookFileBlobUrl, fetchBookFileText } from "./api";
import { rewriteVirtualLinks } from "./virtualLinks";
import type { SerializedNode } from "./types";
import { isComment } from "./types";

const RESOURCE_ATTRS: Record<string, string[]> = {
  img: ["src"],
  link: ["href"],
  image: ["href", "xlink:href"],
  source: ["src"],
};

export interface ResolveContext {
  bookId: string;
  fmt: string;
  size: number;
  mtime: number;
  linkUid: string;
  /** Text-fetched (and, if CSS, url()-rewritten) resources, cached by name so a shared stylesheet isn't fetched twice per page. */
  textCache: Map<string, Promise<string>>;
}

function buildElement(doc: Document, node: SerializedNode, ctx: ResolveContext, pending: Promise<void>[]): Node {
  if (isComment(node)) {
    return doc.createComment(node.x ?? "");
  }
  const el = doc.createElement(node.n);
  for (const [name, value] of node.a ?? []) {
    el.setAttribute(name, value);
  }
  if (node.x) {
    el.appendChild(doc.createTextNode(node.x));
  }
  for (const child of node.c ?? []) {
    el.appendChild(buildElement(doc, child, ctx, pending));
    // `l` (tail) is a sibling text node after the child, per lxml
    // text/tail semantics -- see reader_json.rs's own doc.
    if (child.l) {
      el.appendChild(doc.createTextNode(child.l));
    }
  }

  if (el.tagName === "STYLE" && el.textContent) {
    pending.push(
      resolveCssText(el.textContent, ctx).then((rewritten) => {
        el.textContent = rewritten;
      }),
    );
  }
  const styleAttr = el.getAttribute("style");
  if (styleAttr) {
    pending.push(
      resolveCssText(styleAttr, ctx).then((rewritten) => {
        el.setAttribute("style", rewritten);
      }),
    );
  }
  const resourceAttrs = RESOURCE_ATTRS[el.tagName.toLowerCase()];
  if (resourceAttrs) {
    for (const attr of resourceAttrs) {
      const value = el.getAttribute(attr);
      if (value && value.includes(`${ctx.linkUid}|`)) {
        pending.push(resolveAttrValue(value, ctx).then((resolved) => el.setAttribute(attr, resolved)));
      }
    }
  }

  return el;
}

async function resolveAttrValue(value: string, ctx: ResolveContext): Promise<string> {
  let resolved = value;
  const matches: { name: string; frag: string }[] = [];
  rewriteVirtualLinks(value, ctx.linkUid, (link) => {
    matches.push(link);
    return null; // first pass just collects; real replace happens below once fetches resolve
  });
  for (const { name } of matches) {
    const blobUrl = await fetchBookFileBlobUrl(ctx.bookId, ctx.fmt, ctx.size, ctx.mtime, name);
    resolved = rewriteVirtualLinks(resolved, ctx.linkUid, (link) => (link.name === name ? blobUrl : null));
  }
  return resolved;
}

/** Resolves `url(link_uid|...|)` references inside a block of CSS text (a `<style>` element's own text, or an inline `style=` value), fetching referenced images/fonts as blob URLs. Nested stylesheets are not followed -- `<link>` elements are resolved separately via RESOURCE_ATTRS. */
async function resolveCssText(css: string, ctx: ResolveContext): Promise<string> {
  const names = new Set<string>();
  rewriteVirtualLinks(css, ctx.linkUid, (link) => {
    names.add(link.name);
    return null;
  });
  let resolved = css;
  for (const name of names) {
    const blobUrl = await fetchBookFileBlobUrl(ctx.bookId, ctx.fmt, ctx.size, ctx.mtime, name);
    resolved = rewriteVirtualLinks(resolved, ctx.linkUid, (link) => (link.name === name ? blobUrl : null));
  }
  return resolved;
}

/** Fetches `name`'s reader-json body, unserializes it into `iframeDoc`'s `<html>` root, and resolves every virtualized resource reference. Returns once the DOM is built AND every resource fetch has settled (a caller that wants to show content sooner could await just the DOM-build half instead). */
export async function loadSpineFileInto(iframeDoc: Document, ctx: ResolveContext, name: string): Promise<void> {
  const raw = await fetchBookFileText(ctx.bookId, ctx.fmt, ctx.size, ctx.mtime, name);
  const body = JSON.parse(raw) as { version: number; tree: SerializedNode; ns_map: string[] };

  iframeDoc.open();
  iframeDoc.write("<!doctype html><html><head></head><body></body></html>");
  iframeDoc.close();

  const pending: Promise<void>[] = [];
  const root = buildElement(iframeDoc, body.tree, ctx, pending) as Element;
  const existingHtml = iframeDoc.documentElement;
  if (existingHtml && root.tagName === "HTML") {
    existingHtml.replaceWith(root);
  } else {
    iframeDoc.documentElement.appendChild(root);
  }

  await Promise.all(pending);
}
