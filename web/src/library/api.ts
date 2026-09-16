// Real fetch wrappers against calibre_srv's `/ajax/*` REST API (see
// crates/calibre_srv/src/ajax.rs). Port of old_src/src/pyj/ajax.pyj's
// role for this slice, narrowed to only what the library-browser MVP
// needs.

import type { AddBookResult, BookFieldChanges, BookSummary, BooksInPage, CategoryEntry, CategoryPage, FieldMetadataResponse, SearchResult, VirtualLibraries } from "./types";

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(url, init);
  if (!resp.ok) {
    throw new Error(`${init?.method ?? "GET"} ${url} failed: ${resp.status} ${resp.statusText}`);
  }
  return (await resp.json()) as T;
}

export interface SearchParams {
  query: string;
  num: number;
  offset: number;
  sort: string;
  sortOrder: "asc" | "desc";
  vl: string;
}

export function search(p: SearchParams): Promise<SearchResult> {
  const qs = new URLSearchParams({
    query: p.query,
    num: String(p.num),
    offset: String(p.offset),
    sort: p.sort,
    sort_order: p.sortOrder,
  });
  if (p.vl) qs.set("vl", p.vl);
  return jsonFetch<SearchResult>(`/ajax/search?${qs.toString()}`);
}

export async function fetchBooks(ids: number[]): Promise<BookSummary[]> {
  if (ids.length === 0) return [];
  const data = await jsonFetch<Record<string, BookSummary | null>>(`/ajax/books?ids=${ids.join(",")}`);
  // Preserve the order `ids` (and therefore the page's sort order) was
  // requested in -- the response is an id-keyed object, unordered.
  return ids.map((id) => data[String(id)]).filter((b): b is BookSummary => b !== null && b !== undefined);
}

export function fetchBook(id: number): Promise<BookSummary> {
  return jsonFetch<BookSummary>(`/ajax/book/${id}`);
}

export function fetchCategories(): Promise<CategoryEntry[]> {
  return jsonFetch<CategoryEntry[]>("/ajax/categories");
}

export function fetchCategory(name: string, num = 200): Promise<CategoryPage> {
  return jsonFetch<CategoryPage>(`/ajax/category/${encodeURIComponent(name)}?num=${num}`);
}

export function fetchBooksIn(category: string, itemId: number, num: number, offset: number): Promise<BooksInPage> {
  return jsonFetch<BooksInPage>(`/ajax/books_in/${encodeURIComponent(category)}/${itemId}?num=${num}&offset=${offset}`);
}

export function fetchFieldMetadata(): Promise<FieldMetadataResponse> {
  return jsonFetch<FieldMetadataResponse>("/ajax/field-metadata");
}

export function fetchVirtualLibraries(): Promise<VirtualLibraries> {
  return jsonFetch<VirtualLibraries>("/ajax/virtual-libraries");
}

// Real write endpoints -- see crates/calibre_srv/src/cdb.rs. This app
// only ever runs against a single-library calibre_srv (spawned by the
// desktop app for one chosen library, see app/src-tauri/src/server.rs),
// so the routes below use the no-library_id-segment variants where
// available; `add_book` has no such variant, so a placeholder segment
// is sent (it's ignored server-side in single-library mode).

export async function addBook(file: File, addDuplicates = false): Promise<AddBookResult> {
  const jobId = crypto.randomUUID();
  const url = `/cdb/add-book/${jobId}/${addDuplicates ? "y" : "n"}/${encodeURIComponent(file.name)}/-`;
  return jsonFetch<AddBookResult>(url, { method: "POST", body: file });
}

export async function setFields(bookId: number, changes: BookFieldChanges): Promise<BookSummary> {
  const data = await jsonFetch<Record<string, BookSummary>>(`/cdb/set-fields/${bookId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ changes }),
  });
  return data[String(bookId)];
}

export async function setCover(bookId: number, file: File): Promise<void> {
  await jsonFetch(`/cdb/set-cover/${bookId}`, { method: "POST", body: file });
}

export async function deleteBooks(ids: number[]): Promise<void> {
  if (ids.length === 0) return;
  await jsonFetch(`/cdb/delete-books/${ids.join(",")}`, { method: "POST" });
}

function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error ?? new Error(`failed to read ${file.name}`));
    reader.readAsDataURL(file);
  });
}

export async function addFormat(bookId: number, file: File): Promise<BookSummary> {
  const ext = file.name.split(".").pop()?.toLowerCase() ?? "";
  const dataUrl = await fileToDataUrl(file);
  return setFields(bookId, { added_formats: [{ ext, data_url: dataUrl }] });
}

export function removeFormat(bookId: number, ext: string): Promise<BookSummary> {
  return setFields(bookId, { removed_formats: [ext] });
}
