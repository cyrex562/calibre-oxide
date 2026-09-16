// Real fetch wrappers against calibre_srv's `/ajax/*` REST API (see
// crates/calibre_srv/src/ajax.rs). Port of old_src/src/pyj/ajax.pyj's
// role for this slice, narrowed to only what the library-browser MVP
// needs.

import type { AddBookResult, BookFieldChanges, BookSummary, BooksInPage, CategoryEntry, CategoryPage, ConversionBookData, ConversionStatus, FieldMetadataResponse, FtsSearchResult, FtsSnippet, SearchResult, VirtualLibraries } from "./types";

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

// Real conversion endpoints -- see crates/calibre_srv/src/convert.rs.

export function fetchConversionBookData(bookId: number): Promise<ConversionBookData> {
  return jsonFetch<ConversionBookData>(`/conversion/book-data/${bookId}`);
}

export function startConversion(bookId: number, inputFmt: string, outputFmt: string): Promise<number> {
  return jsonFetch<number>(`/conversion/start/${bookId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ input_fmt: inputFmt, output_fmt: outputFmt }),
  });
}

export function getConversionStatus(jobId: number): Promise<ConversionStatus> {
  return jsonFetch<ConversionStatus>(`/conversion/status/${jobId}`);
}

// Real full-text-search endpoints -- see crates/calibre_srv/src/fts.rs.

export type FtsSearchOutcome = { enabled: true; result: FtsSearchResult } | { enabled: false };

// Distinct from jsonFetch's own generic throw-on-!ok: `/fts/search`
// returning 428 Precondition Required ("full text searching is not
// enabled") is a real, expected outcome the caller needs to
// distinguish from an actual error, not just another failure message.
// (428, not 412 -- ServerError::PreconditionRequired maps to axum's
// StatusCode::PRECONDITION_REQUIRED, RFC 6585's 428; 412 is a
// different status, PRECONDITION_FAILED, not used here. Confirmed
// against a real running calibre_srv, not just the type name.)
export async function ftsSearch(query: string): Promise<FtsSearchOutcome> {
  const resp = await fetch(`/fts/search?${new URLSearchParams({ query })}`);
  if (resp.status === 428) return { enabled: false };
  if (!resp.ok) throw new Error(`GET /fts/search failed: ${resp.status} ${resp.statusText}`);
  return { enabled: true, result: (await resp.json()) as FtsSearchResult };
}

export async function ftsSnippets(bookIds: number[], query: string): Promise<Record<string, FtsSnippet[]>> {
  if (bookIds.length === 0) return {};
  const qs = new URLSearchParams({ query });
  const data = await jsonFetch<{ snippets: Record<string, FtsSnippet[]> }>(`/fts/snippets/${bookIds.join(",")}?${qs}`);
  return data.snippets;
}

// Virtual library / saved search management -- real, new routes (no
// upstream calibre.srv route exists for either; both are GUI-only
// preferences editing there). See crates/calibre_srv/src/lists.rs's
// own doc for why this is a real addition, not a port.

export function fetchSavedSearches(): Promise<Record<string, string>> {
  return jsonFetch<Record<string, string>>("/ajax/saved-searches");
}

async function postNoBody(url: string): Promise<void> {
  const resp = await fetch(url, { method: "POST" });
  if (!resp.ok) throw new Error(`POST ${url} failed: ${resp.status} ${resp.statusText}`);
}

export async function setVirtualLibrary(name: string, query: string): Promise<void> {
  const resp = await fetch(`/vl/set/${encodeURIComponent(name)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
  });
  if (!resp.ok) throw new Error(`POST /vl/set failed: ${resp.status} ${resp.statusText}`);
}

export function deleteVirtualLibrary(name: string): Promise<void> {
  return postNoBody(`/vl/delete/${encodeURIComponent(name)}`);
}

export async function setSavedSearch(name: string, query: string): Promise<void> {
  const resp = await fetch(`/saved-search/set/${encodeURIComponent(name)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
  });
  if (!resp.ok) throw new Error(`POST /saved-search/set failed: ${resp.status} ${resp.statusText}`);
}

export function deleteSavedSearch(name: string): Promise<void> {
  return postNoBody(`/saved-search/delete/${encodeURIComponent(name)}`);
}

export function renameSavedSearch(oldName: string, newName: string): Promise<void> {
  return postNoBody(`/saved-search/rename/${encodeURIComponent(oldName)}/${encodeURIComponent(newName)}`);
}

// Real, new route -- see crates/calibre_srv/src/catalog.rs's own doc
// for why (catalog generation is CLI/GUI-only in real upstream
// calibre, never exposed over HTTP there). A plain GET URL rather
// than a fetch+blob dance: the response's own real Content-Disposition:
// attachment header lets the browser handle the download natively.
export function catalogDownloadUrl(search: string): string {
  const qs = search ? `?${new URLSearchParams({ search })}` : "";
  return `/catalog/generate${qs}`;
}

// Real, new route -- see crates/calibre_srv/src/news.rs's own doc for
// why (fetching news/recipes is CLI/GUI-only in real upstream calibre,
// never exposed over HTTP there). A real, generic RSS/Atom feed
// reader, not a catalog of upstream's ~1077 hand-written per-site
// recipes -- see that module's doc for the real scope.
export interface NewsFetchStatus {
  running: boolean;
  ok?: boolean;
  error?: string;
  book_id?: number;
}

export function startNewsFetch(title: string, feeds: string[]): Promise<number> {
  return jsonFetch<number>("/news/fetch", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ title, feeds }),
  });
}

export function getNewsFetchStatus(jobId: number): Promise<NewsFetchStatus> {
  return jsonFetch<NewsFetchStatus>(`/news/status/${jobId}`);
}

// Unlike this file's other write endpoints, /fts/indexing's real
// handler returns an empty 200 body (`Result<(), ServerError>` in
// Rust), not `{}` -- jsonFetch's own unconditional `.json()` would
// throw on that, so this calls `fetch` directly.
export async function setFtsEnabled(enabled: boolean): Promise<void> {
  const resp = await fetch(`/fts/indexing`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(enabled),
  });
  if (!resp.ok) throw new Error(`POST /fts/indexing failed: ${resp.status} ${resp.statusText}`);
}
