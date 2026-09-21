// Real fetch wrappers against calibre_srv's `/ajax/*` REST API (see
// crates/calibre_srv/src/ajax.rs). Port of old_src/src/pyj/ajax.pyj's
// role for this slice, narrowed to only what the library-browser MVP
// needs.

import type { AddBookResult, BookFieldChanges, BookSummary, BooksInPage, CategoryEntry, CategoryPage, ConversionBookData, ConversionStatus, CustomColumnInfo, FieldMetadataResponse, FtsSearchResult, FtsSnippet, SearchResult, VirtualLibraries } from "./types";

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

export function fileToDataUrl(file: File): Promise<string> {
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

// Real, fixed subset of the ~40 real upstream ConversionOptions fields
// -- see crates/calibre_srv/src/convert.rs's own doc for why this is a
// deliberate slice, not the full set. Omitted fields keep the real
// upstream default server-side.
export type ConversionOptionsOverride = {
  unsmarten_punctuation?: boolean;
  linearize_tables?: boolean;
  insert_metadata?: boolean;
  remove_first_image?: boolean;
  use_auto_toc?: boolean;
  chapter?: string;
  max_toc_links?: number;
  base_font_size?: number;
};

export function startConversion(bookId: number, inputFmt: string, outputFmt: string, options: ConversionOptionsOverride = {}): Promise<number> {
  return jsonFetch<number>(`/conversion/start/${bookId}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ input_fmt: inputFmt, output_fmt: outputFmt, options }),
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

// Real, new route -- see crates/calibre_srv/src/library_export.rs's
// own doc (real upstream's whole-library export is a Qt GUI action,
// never exposed over HTTP there).
export function libraryExportUrl(): string {
  return "/library/export/default";
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

// A feed entry can be a bare URL string, or a real {title, url} object
// naming its own section within the recipe -- see
// crates/calibre_srv/src/news.rs's own FeedInput (issue #765).
export type NewsFeedInput = string | { title: string; url: string };

export interface CustomRecipeOptions {
  /// RecipeConfig::oldest_article override, in days.
  oldestArticleDays?: number;
  /// RecipeConfig::max_articles_per_feed override.
  maxArticlesPerFeed?: number;
}

export function startNewsFetch(title: string, feeds: NewsFeedInput[], options: CustomRecipeOptions = {}): Promise<number> {
  return jsonFetch<number>("/news/fetch", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      title,
      feeds,
      ...(options.oldestArticleDays !== undefined ? { oldest_article_days: options.oldestArticleDays } : {}),
      ...(options.maxArticlesPerFeed !== undefined ? { max_articles_per_feed: options.maxArticlesPerFeed } : {}),
    }),
  });
}

export function getNewsFetchStatus(jobId: number): Promise<NewsFetchStatus> {
  return jsonFetch<NewsFetchStatus>(`/news/status/${jobId}`);
}

// Real, new route -- see crates/calibre_srv/src/share.rs's own doc for
// why (real upstream's own "send to email" feature is desktop-GUI-only,
// never exposed over HTTP there). No persisted SMTP account server-side
// yet (ties into the not-yet-built preferences epic) -- the relay
// config is supplied on every call; LibraryView/BookDetailsPanel is
// expected to remember it client-side (e.g. localStorage) for
// convenience.
export interface SmtpRelayConfig {
  relay: string;
  port?: number;
  username?: string;
  password?: string;
  encryption?: "tls" | "ssl" | "none";
}

export async function shareEmail(bookId: number, format: string, from: string, to: string, relay: SmtpRelayConfig, subject?: string): Promise<void> {
  await jsonFetch("/share/email", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ book_id: bookId, format, from, to, subject, relay }),
  });
}

// Real, new routes -- see crates/calibre_srv/src/custom_columns.rs's
// own doc for why (custom-column management is CLI-layer-only in real
// upstream calibre, and its own remote-command `implementation()` is
// unimplemented there too, never exposed over HTTP either way).
export function fetchCustomColumns(): Promise<Record<string, CustomColumnInfo>> {
  return jsonFetch<Record<string, CustomColumnInfo>>("/custom-columns");
}

export async function addCustomColumn(label: string, name: string, datatype: string): Promise<number> {
  const data = await jsonFetch<{ num: number }>("/custom-columns/add", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label, name, datatype }),
  });
  return data.num;
}

export function removeCustomColumn(label: string): Promise<void> {
  return postNoBody(`/custom-columns/remove/${encodeURIComponent(label)}`);
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

// Real, new routes -- see crates/calibre_srv/src/data_files.rs's own
// doc for why (extra files attached to a book outside its standard
// formats, issue #418, with a real "list what's already attached"
// route added for issue #757 -- upload/remove already returned this
// as a side effect, but nothing could fetch it up front). No
// no-library_id-segment variant exists for these routes (unlike
// set-fields), so this app's own single-library convention ("default")
// is used directly, matching reader/api.ts and library/notes.ts.
const DATA_FILES_LIBRARY_ID = "default";

export interface DataFileStat {
  size: number;
  mtime_ns: number;
}

export function fetchDataFiles(bookId: number): Promise<Record<string, DataFileStat>> {
  return jsonFetch<{ data_files: Record<string, DataFileStat> }>(`/data-files/list/${bookId}/${DATA_FILES_LIBRARY_ID}`).then((r) => r.data_files);
}

export async function uploadDataFile(bookId: number, file: File): Promise<Record<string, DataFileStat>> {
  const dataUrl = await fileToDataUrl(file);
  const r = await jsonFetch<{ error: string; data_files: Record<string, DataFileStat> }>(`/data-files/upload/${bookId}/${DATA_FILES_LIBRARY_ID}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify([{ name: file.name, data_url: dataUrl }]),
  });
  if (r.error) throw new Error(r.error);
  return r.data_files;
}

export async function removeDataFile(bookId: number, relpath: string): Promise<Record<string, DataFileStat>> {
  const r = await jsonFetch<{ data_files: Record<string, DataFileStat>; errors?: Record<string, string> }>(`/data-files/remove/${bookId}/${DATA_FILES_LIBRARY_ID}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify([relpath]),
  });
  if (r.errors && r.errors[relpath]) throw new Error(r.errors[relpath]);
  return r.data_files;
}

// Real, new route -- see crates/calibre_srv/src/template_tester.rs's
// own doc for why (real upstream's Template Tester dialog is Qt
// GUI-only, never exposed over HTTP there) and for the real,
// disclosed narrowing: only Template Program Mode syntax
// (`field('title')`, an optional leading `program:` is stripped) is
// accepted, not the `{field}` shorthand dialect.
export interface TemplateEvalResult {
  ok: boolean;
  result?: string;
  error?: string;
}

export function evaluateTemplate(bookId: number, template: string): Promise<TemplateEvalResult> {
  return jsonFetch<TemplateEvalResult>(`/template-tester/evaluate/${bookId}/default`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ template }),
  });
}

// Real, new route -- see crates/calibre_srv/src/opml.rs's own doc for
// why (no OPML support exists anywhere in this port otherwise). Pairs
// with startNewsFetch above: parses a real OPML export into a flat
// feed list the news-fetch panel can offer to add.
export interface OpmlFeed {
  title: string;
  feed_url: string;
}

export async function importOpml(opml: string): Promise<OpmlFeed[]> {
  const r = await jsonFetch<{ feeds: OpmlFeed[] }>("/opml/import", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ opml }),
  });
  return r.feeds;
}

// Real, new route -- see crates/calibre_srv/src/rename.rs's own doc
// for why (real rename/merge logic already existed but only on a
// LibraryDatabase-compatibility type calibre_srv could never reach
// without opening a second connection to the same library). Renaming
// to an existing item of the same category is a real merge, not an
// error -- the server handles that, this is a plain rename call
// either way.
export async function renameCategoryItem(category: string, itemName: string, newName: string): Promise<void> {
  const resp = await fetch(`/rename-category-item/${encodeURIComponent(category)}/${encodeURIComponent(itemName)}/default`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ new_name: newName }),
  });
  if (!resp.ok) throw new Error((await resp.text()) || `${resp.status} ${resp.statusText}`);
}

// Real, new route -- see crates/calibre_srv/src/check_library.rs's
// own doc for why (real upstream's "Check Library" dialog is Qt
// GUI-only, never exposed over HTTP there).
export interface CheckLibraryFinding {
  a: string;
  b: string;
  book_id: number;
}

export type CheckLibraryResult = Record<string, CheckLibraryFinding[]>;

// Human-readable labels for each real result key, matching
// calibre_db::cli::cmd_check_library's own upstream-derived labels.
export const CHECK_LIBRARY_LABELS: Record<string, string> = {
  invalid_titles: "Invalid titles",
  extra_titles: "Extra titles",
  invalid_authors: "Invalid authors",
  extra_authors: "Extra authors",
  missing_formats: "Missing book formats",
  extra_formats: "Extra book formats",
  extra_files: "Unknown files in books",
  missing_covers: "Missing cover files",
  extra_covers: "Cover files not in database",
  malformed_formats: "Malformed formats",
  malformed_paths: "Malformed book paths",
  corrupted_formats: "Corrupted book formats",
  corrupted_covers: "Corrupted cover files",
};

export function checkLibrary(): Promise<CheckLibraryResult> {
  return jsonFetch<CheckLibraryResult>("/check-library/default", { method: "POST" });
}

// Real, new route -- see crates/calibre_srv/src/save_to_disk.rs's own
// doc (real upstream's "Save to disk" is a Qt GUI action, never
// exposed over HTTP there).
export interface SaveToDiskResult {
  book_id: number;
  ok: boolean;
  paths?: string[];
  error?: string;
}

export function saveToDisk(bookIds: number[], template: string, dest: string, formats?: string[]): Promise<{ results: SaveToDiskResult[] }> {
  return jsonFetch<{ results: SaveToDiskResult[] }>("/save-to-disk/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ book_ids: bookIds, template, dest, ...(formats ? { formats } : {}) }),
  });
}

// Real, new route -- see crates/calibre_srv/src/duplicates.rs's own
// doc (real upstream's "Find duplicates" is a Qt GUI action, never
// exposed over HTTP there).
export interface DuplicateBook {
  book_id: number;
  title: string;
  authors: string[];
}

export function scanForDuplicates(): Promise<{ groups: DuplicateBook[][] }> {
  return jsonFetch<{ groups: DuplicateBook[][] }>("/duplicates/scan/default", { method: "POST" });
}

// Real, new routes -- see crates/calibre_srv/src/news_scheduler.rs's
// own doc (real upstream schedules recurring recipe fetches inside
// its own GUI job scheduler, never exposed over HTTP there).
export interface NewsSchedule {
  id: number;
  title: string;
  feeds: string[];
  interval_secs: number;
  next_run_at: string;
  last_run_at: string | null;
  last_result: string | null;
}

export function addNewsSchedule(title: string, feeds: string[], intervalSecs: number): Promise<{ id: number }> {
  return jsonFetch<{ id: number }>("/news/schedules/add", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ title, feeds, interval_secs: intervalSecs }),
  });
}

export function listNewsSchedules(): Promise<{ schedules: NewsSchedule[] }> {
  return jsonFetch<{ schedules: NewsSchedule[] }>("/news/schedules/list");
}

export function removeNewsSchedule(id: number): Promise<void> {
  return postNoBody(`/news/schedules/remove/${id}`);
}

export function runNewsScheduleNow(id: number): Promise<{ ok: boolean; book_id?: number; error?: string }> {
  return jsonFetch<{ ok: boolean; book_id?: number; error?: string }>(`/news/schedules/run-now/${id}`, { method: "POST" });
}

// Real, new routes -- see crates/calibre_srv/src/metadata_search.rs's
// own doc. Part of the #750 epic: fetch a book's metadata/cover from
// real online sources (Google Books, Open Library) for the user to
// review and merge in, per-field, into the book's own record.
export interface MetadataCandidate {
  source: string;
  title: string | null;
  authors: string[];
  description: string | null;
  publisher: string | null;
  pubdate: string | null;
  tags: string[];
  identifiers: Record<string, string>;
  language: string | null;
  cover_url: string | null;
  rating: number | null;
}

export interface MetadataSearchParams {
  title?: string;
  authors?: string;
  isbn?: string;
}

export function searchMetadataOnline(params: MetadataSearchParams): Promise<{ candidates: MetadataCandidate[]; source_errors: string[] }> {
  return jsonFetch<{ candidates: MetadataCandidate[]; source_errors: string[] }>("/metadata/search", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(params),
  });
}

// Routes a candidate's external cover image URL through the server's
// own cover-proxy (avoids the CORS/mixed-origin issues fetching it
// directly from the browser would hit) -- safe to use directly as an
// <img> src.
export function coverProxyUrl(url: string): string {
  return `/metadata/cover-proxy?url=${encodeURIComponent(url)}`;
}

export async function fetchCoverProxyBlob(url: string): Promise<Blob> {
  const resp = await fetch(coverProxyUrl(url));
  if (!resp.ok) {
    throw new Error(`GET ${coverProxyUrl(url)} failed: ${resp.status} ${resp.statusText}`);
  }
  return resp.blob();
}

export function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error ?? new Error("failed to read blob"));
    reader.readAsDataURL(blob);
  });
}

// Plugin management (#801, closing the #754 epic). See
// crates/calibre_srv/src/plugins.rs.
//
// `capabilities` is the security-relevant part: a plugin runs in a
// WASM sandbox with no filesystem and no network unless its own
// manifest declares them, so the UI shows exactly what a plugin is
// asking for -- before install, via `inspectPlugin`.
export interface PluginCapabilities {
  allowed_hosts: string[];
  allowed_paths: Record<string, string>;
  fully_sandboxed: boolean;
}

export interface InstalledPlugin {
  name: string;
  version: string;
  author: string;
  description: string;
  plugin_type: string;
  file_types: string[];
  enabled: boolean;
  capabilities: PluginCapabilities;
  limits: { timeout_ms: number; max_pages: number };
}

export function listPlugins(): Promise<{ plugins: InstalledPlugin[] }> {
  return jsonFetch<{ plugins: InstalledPlugin[] }>("/plugins/list");
}

// Reads a package's manifest WITHOUT installing it, so the user can
// see what it wants before granting it.
export function inspectPlugin(path: string): Promise<InstalledPlugin> {
  return jsonFetch<InstalledPlugin>("/plugins/inspect", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

export function installPlugin(path: string): Promise<InstalledPlugin> {
  return jsonFetch<InstalledPlugin>("/plugins/install", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

export function removePlugin(name: string): Promise<void> {
  return postNoBody(`/plugins/remove/${encodeURIComponent(name)}`);
}

export function setPluginEnabled(name: string, enabled: boolean): Promise<{ ok: boolean; enabled: boolean }> {
  return jsonFetch<{ ok: boolean; enabled: boolean }>(`/plugins/set-enabled/${encodeURIComponent(name)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ enabled }),
  });
}

// Author/tag mapping (#816 item 1.7). Preview and apply run the same
// computation server-side; only `applyMapper` writes.
export interface MapperRule {
  action: string;
  query: string;
  replace?: string;
  match_type: string;
}

export interface MappedBook {
  book_id: number;
  title: string;
  before: string[];
  after: string[];
}

export interface MapperResult {
  changed: number;
  books: MappedBook[];
}

function mapperBody(field: "authors" | "tags", rules: MapperRule[], bookIds: number[]) {
  return JSON.stringify({ field, rules, book_ids: bookIds });
}

export function previewMapper(field: "authors" | "tags", rules: MapperRule[], bookIds: number[] = []): Promise<MapperResult> {
  return jsonFetch<MapperResult>("/mapper/preview", { method: "POST", headers: { "Content-Type": "application/json" }, body: mapperBody(field, rules, bookIds) });
}

export function applyMapper(field: "authors" | "tags", rules: MapperRule[], bookIds: number[] = []): Promise<MapperResult> {
  return jsonFetch<MapperResult>("/mapper/apply", { method: "POST", headers: { "Content-Type": "application/json" }, body: mapperBody(field, rules, bookIds) });
}

// Library-wide annotation browser (#816 item 1.8). Per-book
// annotations were always reachable; this answers "what have I
// highlighted across the whole library".
export interface LibraryAnnotation {
  id: number;
  book_id: number;
  title: string;
  format: string;
  text: string;
  type: string;
  timestamp: string | null;
}

export async function fetchAllAnnotations(opts: { type?: string; bookIds?: number[]; limit?: number } = {}): Promise<LibraryAnnotation[]> {
  const params = new URLSearchParams();
  if (opts.type) params.set("type", opts.type);
  if (opts.bookIds?.length) params.set("book_ids", opts.bookIds.join(","));
  if (opts.limit) params.set("limit", String(opts.limit));
  const qs = params.toString();
  const data = await jsonFetch<{ count: number; annotations: LibraryAnnotation[] }>(`/annotations/all${qs ? `?${qs}` : ""}`);
  return data.annotations;
}

// Polish (#816 item 1.10). `oeb::polish` is 92 files of merged engine
// that had no caller at all until POST /polish.
export interface PolishOptions {
  jacket: boolean;
  remove_jacket: boolean;
  smarten_punctuation: boolean;
  remove_unused_css: boolean;
  compress_images: boolean;
  upgrade_book: boolean;
  add_soft_hyphens: boolean;
  remove_soft_hyphens: boolean;
  download_external_resources: boolean;
  embed: boolean;
  subset: boolean;
  remove_unused_classes: boolean;
  merge_identical_selectors: boolean;
  merge_rules_with_identical_properties: boolean;
  remove_unreferenced_sheets: boolean;
  remove_ncx: boolean;
}

export interface PolishBookResult {
  book_id: number;
  changed: boolean;
  report?: string[];
  error?: string;
}

export interface PolishResult {
  changed: number;
  failed: number;
  books: PolishBookResult[];
}

export function polishBooks(bookIds: number[], options: Partial<PolishOptions>): Promise<PolishResult> {
  return jsonFetch<PolishResult>("/polish", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ book_ids: bookIds, options }),
  });
}

// Plugin catalog (#816 item 1.14). A plain directory of installable
// packages -- a repo folder or git submodule, not a hosted index:
// plugins for this port must be written against its WASM ABI rather
// than carried over from calibre's Python ones.
export interface CatalogPlugin extends InstalledPlugin {
  installed: boolean;
  installed_version: string | null;
  update_available: boolean;
}

export async function fetchPluginCatalog(): Promise<{ configured: boolean; plugins: CatalogPlugin[] }> {
  return jsonFetch<{ configured: boolean; plugins: CatalogPlugin[] }>("/plugins/catalog");
}

export function installFromCatalog(name: string): Promise<InstalledPlugin> {
  return jsonFetch<InstalledPlugin>(`/plugins/install-from-catalog/${encodeURIComponent(name)}`, { method: "POST" });
}

// Editor check-book and reports (#816 items 3.3 / 3.2). Both engines
// were fully ported with no caller; these routes act on the open
// tweak session, so they see unsaved edits.
export interface CheckItem {
  type: string;
  message: string;
  file: string;
  line: number | null;
  col: number | null;
  level: string;
  help: string;
  fixable: boolean;
  fix_label: string | null;
}

export interface CheckResult {
  count: number;
  errors: number;
  fixable: number;
  items: CheckItem[];
}

export interface BookReport {
  files: { count: number; total_size: number; items: { name: string; category: string; size: number; words: number }[] };
  images: { count: number; items: { name: string; size: number; width: number; height: number; usage: number }[] };
}

export function checkBook(sessionId: string): Promise<CheckResult> {
  return jsonFetch<CheckResult>(`/tweak/check/${encodeURIComponent(sessionId)}`);
}

export function fixBookChecks(sessionId: string): Promise<{ attempted: number; changed: boolean }> {
  return jsonFetch<{ attempted: number; changed: boolean }>(`/tweak/check-fix/${encodeURIComponent(sessionId)}`, { method: "POST" });
}

export function bookReport(sessionId: string): Promise<BookReport> {
  return jsonFetch<BookReport>(`/tweak/report/${encodeURIComponent(sessionId)}`);
}

// Spell check (#816 item 3.1). The engine was always real; what was
// missing was dictionary data, which now ships with the binary.
export interface MisspelledWord {
  word: string;
  count: number;
  files: string[];
  suggestions: string[];
}

export function spellCheckBook(sessionId: string): Promise<{ count: number; words: MisspelledWord[] }> {
  return jsonFetch<{ count: number; words: MisspelledWord[] }>(`/tweak/spell/${encodeURIComponent(sessionId)}`);
}

// Search and replace across the book (#816 item 3.4). Acts on the
// open tweak session, so a replace is part of the same
// commit-or-discard decision as any other editor change.
export interface SearchHit {
  line: number;
  text: string;
  context: string;
}

export interface SearchFile {
  name: string;
  count: number;
  samples: SearchHit[];
}

export interface SearchReplaceResult {
  matches: number;
  files: SearchFile[];
  replaced: boolean;
  changed_files: number;
}

export function searchReplaceBook(
  sessionId: string,
  opts: { find: string; replace?: string; regex?: boolean; caseSensitive?: boolean; dryRun?: boolean },
): Promise<SearchReplaceResult> {
  return jsonFetch<SearchReplaceResult>(`/tweak/search-replace/${encodeURIComponent(sessionId)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ find: opts.find, replace: opts.replace, regex: !!opts.regex, case_sensitive: !!opts.caseSensitive, dry_run: !!opts.dryRun }),
  });
}
