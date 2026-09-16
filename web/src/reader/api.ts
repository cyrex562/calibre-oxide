// Real fetch wrappers against calibre_srv's already-real endpoints
// (calibre_srv::render_endpoints / calibre_srv::books). Port of
// old_src/src/pyj/ajax.pyj's role for this slice, narrowed to only
// what the reader MVP needs (no generic XHR-progress/upload support).

import type { Bookmark, BookManifest, Highlight, LastReadPosition } from "./types";

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(url, init);
  if (!resp.ok) {
    throw new Error(`${init?.method ?? "GET"} ${url} failed: ${resp.status} ${resp.statusText}`);
  }
  return (await resp.json()) as T;
}

export function fetchManifest(bookId: string, fmt: string, forceReload = false): Promise<BookManifest> {
  const q = forceReload ? "?force_reload=1" : "";
  return jsonFetch<BookManifest>(`/book-manifest/${encodeURIComponent(bookId)}/${encodeURIComponent(fmt)}${q}`);
}

/// Percent-encodes each path segment of `name` individually and
/// rejoins with literal `/` -- matching how axum's `{*name}` wildcard
/// route actually needs its path built (each segment decoded, `/`
/// itself is the segment boundary). The old RapydScript client instead
/// encodes the whole name then un-escapes `%2F` back to `/` via
/// regex; this is the more direct equivalent, not a re-implementation
/// of that regex hack.
function encodeFileName(name: string): string {
  return name.split("/").map(encodeURIComponent).join("/");
}

export function bookFileUrl(bookId: string, fmt: string, size: number, mtime: number, name: string): string {
  return `/book-file/${encodeURIComponent(bookId)}/${encodeURIComponent(fmt)}/${size}/${mtime}/${encodeFileName(name)}`;
}

export async function fetchBookFileText(bookId: string, fmt: string, size: number, mtime: number, name: string): Promise<string> {
  const resp = await fetch(bookFileUrl(bookId, fmt, size, mtime, name));
  if (!resp.ok) {
    throw new Error(`book-file ${name} failed: ${resp.status}`);
  }
  return resp.text();
}

export async function fetchBookFileBlobUrl(bookId: string, fmt: string, size: number, mtime: number, name: string): Promise<string> {
  const resp = await fetch(bookFileUrl(bookId, fmt, size, mtime, name));
  if (!resp.ok) {
    throw new Error(`book-file ${name} failed: ${resp.status}`);
  }
  const blob = await resp.blob();
  return URL.createObjectURL(blob);
}

const LIBRARY_ID = "default";

export async function getLastReadPositions(bookId: string, fmt: string): Promise<LastReadPosition[]> {
  const which = `${bookId}-${fmt}`;
  const data = await jsonFetch<Record<string, LastReadPosition[]>>(`/book-get-last-read-position/${LIBRARY_ID}/${which}`);
  return data[`${bookId}:${fmt}`] ?? [];
}

export async function setLastReadPosition(bookId: string, fmt: string, device: string, cfi: string | null, posFrac: number): Promise<void> {
  await fetch(`/book-set-last-read-position/${LIBRARY_ID}/${bookId}/${fmt}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ device, cfi, pos_frac: posFrac }),
  });
}

export async function getAnnotations(bookId: string, fmt: string): Promise<{ bookmark?: Bookmark[]; highlight?: Highlight[] }> {
  const which = `${bookId}-${fmt}`;
  const data = await jsonFetch<Record<string, { annotations_map?: { bookmark?: Bookmark[]; highlight?: Highlight[] } }>>(`/book-get-annotations/${LIBRARY_ID}/${which}`);
  return data[`${bookId}:${fmt}`]?.annotations_map ?? {};
}

export async function addBookmark(bookId: string, fmt: string, title: string, pos: string): Promise<Bookmark> {
  const bookmark: Bookmark = { type: "bookmark", title, timestamp: new Date().toISOString(), pos, pos_type: "calibre-oxide-simple-pos" };
  const resp = await fetch(`/book-update-annotations/${LIBRARY_ID}/${bookId}/${fmt}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ bookmark: [bookmark] }),
  });
  if (!resp.ok) throw new Error(`POST /book-update-annotations failed: ${resp.status} ${resp.statusText}`);
  return bookmark;
}

export async function addHighlight(bookId: string, fmt: string, startCfi: string, endCfi: string, highlightedText: string): Promise<Highlight> {
  const highlight: Highlight = { type: "highlight", uuid: crypto.randomUUID(), timestamp: new Date().toISOString(), start_cfi: startCfi, end_cfi: endCfi, highlighted_text: highlightedText };
  const resp = await fetch(`/book-update-annotations/${LIBRARY_ID}/${bookId}/${fmt}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ highlight: [highlight] }),
  });
  if (!resp.ok) throw new Error(`POST /book-update-annotations failed: ${resp.status} ${resp.statusText}`);
  return highlight;
}
