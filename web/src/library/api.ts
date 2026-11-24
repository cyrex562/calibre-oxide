// Real fetch wrappers against lexmechanic_srv's `/ajax/*` REST API (see
// crates/lexmechanic_srv/src/ajax.rs). Port of old_src/src/pyj/ajax.pyj's
// role for this slice, narrowed to only what the library-browser MVP
// needs.

import { markLibraryChanged } from "./libraryEpoch";
import type { AbsorbReport, AbsorbableList } from "./absorb";
import { BatchUnsupportedError, type BatchGroup, type BatchResponse } from "./bulkBatch";
import { parseChangeLogStatus, type ChangeLogStatus } from "./changeLog";
import { importBody, type ImportFolderRequest, type ImportReport } from "./importFolder";
import type { UntrackedCount } from "./scanPrompt";
import { parseStorageInfo, type StorageInfo } from "./storageBanner";
import type { RestoreRequest, TrashListing } from "./trash";
import type { AddBookResult, BookFieldChanges, BookSummary, BooksInPage, CategoryEntry, CategoryPage, ConversionBookData, ConversionStatus, CustomColumnInfo, EmbedMetadataResponse, FieldMetadataResponse, FtsSearchResult, FtsSnippet, RenameFilesResponse, SearchResult, VirtualLibraries } from "./types";

/**
 * What the server said about a failure, when it said something short: a
 * refusal such as "that folder is inside the library already" is the whole
 * explanation, and "400 Bad Request" alone throws it away.
 */
async function failureDetail(resp: Response): Promise<string> {
  try {
    if (typeof resp.text !== "function") return "";
    const text = (await resp.text()).trim();
    return text && text.length <= 300 && !text.startsWith("<") && !text.startsWith("{") ? ` — ${text}` : "";
  } catch {
    return "";
  }
}

async function jsonFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(url, init);
  if (!resp.ok) {
    throw new Error(`${init?.method ?? "GET"} ${url} failed: ${resp.status} ${resp.statusText}${await failureDetail(resp)}`);
  }
  // A write: a library view kept alive behind the reader reloads when shown again.
  if (init?.method && init.method.toUpperCase() !== "GET") markLibraryChanged();
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
  // `files=1` adds the Size, Folder and File name fields the optional table columns show.
  const data = await jsonFetch<Record<string, BookSummary | null>>(`/ajax/books?ids=${ids.join(",")}&files=1`);
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

/** Every value of a field with its book count, most-used first (feeds the typing suggestions). */
export function fetchCategoryValues(field: string): Promise<CategoryPage> {
  return jsonFetch<CategoryPage>(`/ajax/category/${encodeURIComponent(field)}?num=1000000&offset=0&sort=popularity&sort_order=desc`);
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

// Real write endpoints -- see crates/lexmechanic_srv/src/cdb.rs. This app
// only ever runs against a single-library lexmechanic_srv (spawned by the
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

/**
 * Applies field changes to many books in one request (bulk edit), as groups of
 * books that share a change set. A server without the route answers 404 and
 * this throws [`BatchUnsupportedError`] so the caller can go book by book.
 */
export async function setFieldsBatch(groups: BatchGroup[]): Promise<BatchResponse> {
  const resp = await fetch("/cdb/set-fields-batch/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ groups }),
  });
  if (resp.status === 404 || resp.status === 405) throw new BatchUnsupportedError();
  if (!resp.ok) throw new Error(`POST /cdb/set-fields-batch failed: ${resp.status} ${resp.statusText}${await failureDetail(resp)}`);
  return (await resp.json()) as BatchResponse;
}

/** `GET /change-log/status/{lib}`: whether the library's change log missed changes. Null from an older server. */
export async function fetchChangeLogStatus(): Promise<ChangeLogStatus | null> {
  const resp = await fetch("/change-log/status/default");
  if (!resp.ok) return null;
  return parseChangeLogStatus(await resp.json());
}

/** `POST /change-log/repair/{lib}`: makes the log whole again, then clears the warning. Returns the new status. */
export async function repairChangeLog(): Promise<ChangeLogStatus | null> {
  const data = await jsonFetch<unknown>("/change-log/repair/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{}",
  });
  return parseChangeLogStatus(data);
}

export async function setCover(bookId: number, file: File): Promise<void> {
  await jsonFetch(`/cdb/set-cover/${bookId}`, { method: "POST", body: file });
}

/**
 * Renames the files of one or more books, or -- with `dryRun` -- says
 * what renaming them would do.
 *
 * `template` is a calibre `{field}` template, the same dialect
 * save-to-disk uses; a string with no substitutions in it is a literal
 * name, which is how a single book gets renamed by hand.
 *
 * Preview and apply are the same request deliberately: the names shown
 * in a preview are then produced by exactly the code that will use
 * them, collision handling included.
 */
export async function renameFiles(bookIds: number[], template: string, dryRun: boolean): Promise<RenameFilesResponse> {
  return await jsonFetch<RenameFilesResponse>("/rename-files/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ book_ids: bookIds, template, dry_run: dryRun }),
  });
}

/**
 * Renders a page of the book's PDF on the server and stores it as the
 * cover.
 *
 * The reader does this in the browser instead -- it already has the
 * page rendered, and posting those pixels to `setCover` is one step
 * rather than two. This is for the book nobody has open: a PDF added
 * before covers were generated, or one whose first page is blank.
 */
export async function coverFromPdfPage(bookId: number, page = 1): Promise<void> {
  await jsonFetch(`/cdb/cover-from-pdf-page/${bookId}/${page}`, { method: "POST" });
}

/**
 * Writes the library's own metadata into the book files themselves, so a
 * correction made here reaches the file a reader app opens (#958).
 *
 * Reports per format rather than resolving to nothing: a book can have
 * one format written and another skipped for want of a writer, and the
 * caller has to be able to say which. `status` is `embedded`,
 * `no_writer`, `file_missing` or `failed`, with a `detail` for all but
 * the first.
 */
export async function embedMetadata(ids: number[]): Promise<EmbedMetadataResponse> {
  if (ids.length === 0) return { books: {} };
  return await jsonFetch<EmbedMetadataResponse>(`/cdb/embed-metadata/${ids.join(",")}`, { method: "POST" });
}

/**
 * Flattens an embed run into one line per outcome worth telling the user
 * about, newest concern first: a book that could not be read at all,
 * then a format that failed, then one with no writer. Formats that were
 * written are counted rather than listed -- "3 files updated" is more
 * use than three identical lines.
 */
export function summarizeEmbed(response: EmbedMetadataResponse): { embedded: number; problems: string[] } {
  let embedded = 0;
  const failed: string[] = [];
  const missing: string[] = [];
  const unwritable: string[] = [];
  const unreadable: string[] = [];

  for (const [bookId, outcome] of Object.entries(response.books)) {
    if (outcome.error) {
      unreadable.push(`Book ${bookId}: ${outcome.error}`);
      continue;
    }
    for (const [format, result] of Object.entries(outcome.formats ?? {})) {
      if (result.status === "embedded") embedded += 1;
      else if (result.status === "failed") failed.push(`Book ${bookId} (${format}): ${result.detail ?? "failed"}`);
      else if (result.status === "file_missing") missing.push(`Book ${bookId} (${format}): file missing`);
      else unwritable.push(`Book ${bookId}: no metadata writer for ${format} yet`);
    }
  }

  return { embedded, problems: [...unreadable, ...failed, ...missing, ...unwritable] };
}

/** What a delete did: the books moved to the library Trash, and the ones that could not be. */
export interface DeleteOutcome {
  deleted: number[];
  failed: { book_id: number; error: string }[];
}

/**
 * Normalizes the server's answer. A server that has the partial-failure
 * reply sends `{deleted, failed}`; an older one sends something else
 * (or nothing) and a 2xx meant every book went.
 */
export function parseDeleteOutcome(ids: number[], body: unknown): DeleteOutcome {
  const b = (body ?? {}) as { deleted?: unknown; failed?: unknown };
  if (!Array.isArray(b.deleted) && !Array.isArray(b.failed)) return { deleted: [...ids], failed: [] };
  const failed = (Array.isArray(b.failed) ? b.failed : []).map((f) => {
    const e = f as { book_id?: number; error?: string };
    return { book_id: Number(e.book_id), error: String(e.error ?? "failed") };
  });
  const deleted = Array.isArray(b.deleted)
    ? (b.deleted as unknown[]).map(Number)
    : ids.filter((id) => !failed.some((f) => f.book_id === id));
  return { deleted, failed };
}

/** One line for the failures of a delete, or "" when there were none. */
export function describeDeleteFailures(outcome: DeleteOutcome): string {
  if (outcome.failed.length === 0) return "";
  const shown = outcome.failed.slice(0, 3).map((f) => `book ${f.book_id}: ${f.error}`).join("; ");
  const more = outcome.failed.length > 3 ? ` (and ${outcome.failed.length - 3} more)` : "";
  return `${outcome.failed.length} could not be deleted: ${shown}${more}`;
}

export async function deleteBooks(ids: number[]): Promise<DeleteOutcome> {
  if (ids.length === 0) return { deleted: [], failed: [] };
  const body = await jsonFetch<unknown>(`/cdb/delete-books/${ids.join(",")}`, { method: "POST" });
  return parseDeleteOutcome(ids, body);
}

/**
 * Puts books back from the library Trash (the undo of a delete): finds
 * their whole-book entries and restores each. Returns how many came back.
 */
export async function restoreDeletedBooks(ids: number[]): Promise<number> {
  const wanted = new Set(ids);
  const listing = await fetchTrash();
  // A book deleted more than once has several entries: the newest is the one just made.
  const newest = new Map<number, (typeof listing.entries)[number]>();
  for (const entry of listing.entries) {
    if (entry.kind !== "book" || !wanted.has(entry.book_id)) continue;
    const seen = newest.get(entry.book_id);
    if (!seen || entry.deleted_at > seen.deleted_at) newest.set(entry.book_id, entry);
  }
  for (const entry of newest.values()) await restoreTrashEntry({ kind: "book", key: entry.key });
  return newest.size;
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
  // The name travels too: since #893 a format file keeps the name it
  // arrived with rather than being renamed to the book's title.
  return setFields(bookId, { added_formats: [{ ext, data_url: dataUrl, name: file.name }] });
}

export function removeFormat(bookId: number, ext: string): Promise<BookSummary> {
  return setFields(bookId, { removed_formats: [ext] });
}

// Real conversion endpoints -- see crates/lexmechanic_srv/src/convert.rs.

export function fetchConversionBookData(bookId: number): Promise<ConversionBookData> {
  return jsonFetch<ConversionBookData>(`/conversion/book-data/${bookId}`);
}

// Real, fixed subset of the ~40 real upstream ConversionOptions fields
// -- see crates/lexmechanic_srv/src/convert.rs's own doc for why this is a
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

// Real full-text-search endpoints -- see crates/lexmechanic_srv/src/fts.rs.

export type FtsSearchOutcome = { enabled: true; result: FtsSearchResult } | { enabled: false };

// Distinct from jsonFetch's own generic throw-on-!ok: `/fts/search`
// returning 428 Precondition Required ("full text searching is not
// enabled") is a real, expected outcome the caller needs to
// distinguish from an actual error, not just another failure message.
// (428, not 412 -- ServerError::PreconditionRequired maps to axum's
// StatusCode::PRECONDITION_REQUIRED, RFC 6585's 428; 412 is a
// different status, PRECONDITION_FAILED, not used here. Confirmed
// against a real running lexmechanic_srv, not just the type name.)
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
// preferences editing there). See crates/lexmechanic_srv/src/lists.rs's
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

// Real, new route -- see crates/lexmechanic_srv/src/catalog.rs's own doc
// for why (catalog generation is CLI/GUI-only in real upstream
// calibre, never exposed over HTTP there). A plain GET URL rather
// than a fetch+blob dance: the response's own real Content-Disposition:
// attachment header lets the browser handle the download natively.
export function catalogDownloadUrl(search: string): string {
  const qs = search ? `?${new URLSearchParams({ search })}` : "";
  return `/catalog/generate${qs}`;
}

// Real, new route -- see crates/lexmechanic_srv/src/library_export.rs's
// own doc (real upstream's whole-library export is a Qt GUI action,
// never exposed over HTTP there).
export function libraryExportUrl(): string {
  return "/library/export/default";
}

// Real, new route -- see crates/lexmechanic_srv/src/news.rs's own doc for
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
// crates/lexmechanic_srv/src/news.rs's own FeedInput (issue #765).
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

// Real, new route -- see crates/lexmechanic_srv/src/share.rs's own doc for
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

// Real, new routes -- see crates/lexmechanic_srv/src/custom_columns.rs's
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

// Real, new routes -- see crates/lexmechanic_srv/src/data_files.rs's own
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

// Real, new route -- see crates/lexmechanic_srv/src/template_tester.rs's
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

// Real, new route -- see crates/lexmechanic_srv/src/opml.rs's own doc for
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

// Real, new route -- see crates/lexmechanic_srv/src/rename.rs's own doc
// for why (real rename/merge logic already existed but only on a
// LibraryDatabase-compatibility type lexmechanic_srv could never reach
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

// Real, new route -- see crates/lexmechanic_srv/src/check_library.rs's
// own doc for why (real upstream's "Check Library" dialog is Qt
// GUI-only, never exposed over HTTP there).
export interface CheckLibraryFinding {
  /** Null for a finding about a file rather than a book. */
  book_id: number | null;
  title: string;
  /** Relative to the library root. */
  path: string;
}

/**
 * Category name to findings, plus two booleans.
 *
 * `conclusive` is false when a folder could not be read — `missing_files`
 * is then not computed at all, and the UI has to say so rather than
 * implying nothing is missing.
 */
export type CheckLibraryResult = Record<string, CheckLibraryFinding[] | boolean>;

// Human-readable labels for each real result key, matching
// lexmechanic_db::cli::cmd_check_library's own upstream-derived labels.
/**
 * What the library check reports, in the order it is worth reading (#897).
 *
 * The categories changed with the check itself: the old ones described
 * calibre's `<Author>/<Title (id)>` folder convention, which this project
 * no longer maintains, and called every edited file a corrupted one.
 *
 * "Changed since the last check" is deliberately not "corrupted". A file
 * whose content differs might be an edit or bit rot, and nothing can tell
 * those apart — claiming damage asserts something unknowable, and the
 * person who knows whether they opened it is the user.
 */
export const CHECK_LIBRARY_LABELS: Record<string, string> = {
  missing_files: "Files that are gone",
  orphaned_books: "Books waiting to be relocated",
  untracked_files: "Books in the folder that are not in the library",
  unreadable_folders: "Folders that could not be read",
  missing_covers: "Covers that are gone",
  moved_files: "Files found somewhere new",
  changed_files: "Changed since the last check",
  ignored_files: "Removed from the library, kept on disk",
  nested_libraries: "Folders skipped — they are libraries of their own",
};

export function checkLibrary(): Promise<CheckLibraryResult> {
  return jsonFetch<CheckLibraryResult>("/check-library/default", { method: "POST" });
}

/**
 * The result of bringing the index up to date with the folder.
 *
 * Distinct from `checkLibrary`, which only reports: this one writes.
 */
export interface ScanLibraryResult {
  /** Ids of books created from files nothing claimed. */
  added: number[];
  already_known: number;
  /**
   * Records corrected to where the file actually is. Not a problem —
   * moving a file inside your own folder is allowed.
   */
  relocated: number;
  /** Removed from the library but kept on disk, so deliberately skipped. */
  ignored: string[];
  /** Still being written. The next scan picks them up. */
  settling: string[];
  /** Cloud-sync placeholders, not downloaded. */
  offline: string[];
  nested_libraries: string[];
  failed: { path: string; error: string }[];
  /**
   * False when a folder could not be read, so the scan saw only part of
   * the library. What was added is still right; what is absent is
   * unknown, and the UI must not imply the library is now in sync.
   */
  conclusive: boolean;
  unreadable: string[];
  /** The job was cancelled part-way; what was added is kept and a rerun continues. */
  cancelled?: boolean;
}

/**
 * Live counters of a background library job. Which ones move depends on
 * the job: a scan counts `seen` then `added`, the metadata pass
 * `enriched`, the hash backfill `hashed`.
 */
export interface LibraryJobProgress {
  /** "walking" | "indexing" | "enriching" | "hashing" */
  phase: string;
  cancelled: boolean;
  total: number;
  seen: number;
  hashed: number;
  added: number;
  already_known: number;
  ignored: number;
  enriched: number;
  errors: number;
  /** PDFs given a page/text fingerprint. */
  fingerprinted?: number;
  bytes_hashed: number;
}

export interface LibraryJob<R = unknown> {
  id: number;
  kind: "scan" | "enrich" | "hash_backfill" | "pdf_fingerprint" | "merge_exact" | "import" | "dedupe" | "resolve" | "prepare" | "absorb" | "unabsorb" | "copy" | "organize" | "undo_organize" | "retitle" | "undo_retitle" | "mirror";
  /** `interrupted`: the server process ended while it ran (read back from `.lexmechanic/jobs.json`). */
  state: "running" | "finished" | "cancelled" | "failed" | "interrupted";
  /** Interrupted and able to continue by itself: POST `/<resume_route>/default`. */
  resumable?: boolean;
  resume_route?: string | null;
  progress: LibraryJobProgress;
  /** Present once the job has ended. */
  report: R | null;
  error: string | null;
}

export interface LibraryJobHandlers<R = unknown> {
  /** Called once with the job's id, so the caller can offer Cancel. */
  onStart?: (jobId: number) => void;
  /** Called with every status poll. */
  onProgress?: (job: LibraryJob<R>) => void;
  /** Milliseconds between polls. */
  intervalMs?: number;
}

/** Every job the server knows of, newest first (including ones a previous process left interrupted). */
export async function fetchLibraryJobs(): Promise<LibraryJob[]> {
  return (await jsonFetch<{ jobs: LibraryJob[] }>("/library-jobs")).jobs;
}

/** The job running now, if any. */
export async function runningLibraryJob(): Promise<LibraryJob | null> {
  return (await fetchLibraryJobs()).find((j) => j.state === "running") ?? null;
}

/** The newest interrupted job that can be resumed, unless a newer job of the same kind has run since (jobs are newest first). */
export function resumableJob(jobs: LibraryJob[]): LibraryJob | null {
  const idx = jobs.findIndex((j) => j.state === "interrupted" && j.resumable);
  if (idx < 0) return null;
  const superseded = jobs.slice(0, idx).some((j) => j.kind === jobs[idx].kind);
  return superseded ? null : jobs[idx];
}

/**
 * Whether switching library should ask first, and what to say: a running
 * job is stopped when the server for this library is replaced.
 */
export function switchWarning(running: LibraryJob | null): string | null {
  if (!running) return null;
  return `A ${running.kind.replace(/_/g, " ")} job is still running in this library. Switching library stops it (it can be resumed later). Switch anyway?`;
}

/** Continues an interrupted job by starting its resume route, and follows it. */
export function resumeLibraryJob(job: LibraryJob, handlers: LibraryJobHandlers = {}): Promise<unknown> {
  if (!job.resume_route) return Promise.reject(new Error(`a ${job.kind} job cannot be resumed from here`));
  return runLibraryJob<unknown>(`/${job.resume_route}/default`, handlers);
}

export function fetchLibraryJob<R = unknown>(jobId: number): Promise<LibraryJob<R>> {
  return jsonFetch<LibraryJob<R>>(`/library-jobs/${jobId}`);
}

/** Asks a running job to stop. It stops at its next read chunk and keeps what it did. */
export function cancelLibraryJob(jobId: number): Promise<LibraryJob> {
  return jsonFetch<LibraryJob>(`/library-jobs/${jobId}/cancel`, { method: "POST" });
}

/**
 * Starts a library job and follows it to the end.
 *
 * The routes answer at once with the job's id, because a scan of a large
 * folder takes far longer than a request should be held open; this polls
 * the status route and resolves with the job's final report. A cancelled
 * job resolves (its report says `cancelled`); a failed one rejects.
 */
async function runLibraryJob<R>(startUrl: string, handlers: LibraryJobHandlers<R>, body?: unknown): Promise<R> {
  const init: RequestInit = body === undefined ? { method: "POST" } : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  const started = await jsonFetch<{ job_id: number }>(startUrl, init);
  handlers.onStart?.(started.job_id);
  // A single failed poll (a dropped connection, a busy server) must not
  // abandon a job that is still running; give up after three in a row.
  let failedPolls = 0;
  for (;;) {
    let job: LibraryJob<R>;
    try {
      job = await fetchLibraryJob<R>(started.job_id);
      failedPolls = 0;
    } catch (e) {
      if (++failedPolls >= 3) throw e;
      await new Promise((resolve) => setTimeout(resolve, handlers.intervalMs ?? 500));
      continue;
    }
    handlers.onProgress?.(job);
    if (job.state === "failed") throw new Error(job.error ?? "the job failed");
    if (job.state !== "running") return job.report as R;
    await new Promise((resolve) => setTimeout(resolve, handlers.intervalMs ?? 500));
  }
}

export function scanLibrary(handlers: LibraryJobHandlers<ScanLibraryResult> = {}): Promise<ScanLibraryResult> {
  return runLibraryJob<ScanLibraryResult>("/scan-library/default", handlers);
}

/** What the hash backfill did. */
export interface HashBackfillResult {
  /** Files that now have a content hash. */
  hashed: number;
  /** Files that had none when the job started. */
  total: number;
  /** Listed in the library but not on disk. Reported, not fatal. */
  missing: string[];
  missing_count: number;
  unreadable: [string, string][];
  unreadable_count: number;
  cancelled: boolean;
}

/**
 * Gives every book file in the library a content hash, which is what lets
 * "Find duplicates" see identical files in a library that was never
 * scanned here (every library opened from real calibre).
 */
export function backfillHashes(handlers: LibraryJobHandlers<HashBackfillResult> = {}): Promise<HashBackfillResult> {
  return runLibraryJob<HashBackfillResult>("/hash-backfill/default", handlers);
}

/** The stages of the prepare chain, in order. */
export const PREPARE_STAGES = ["scan", "hash", "fingerprint", "dedupe"] as const;
export type PrepareStage = (typeof PREPARE_STAGES)[number];

/** What the chained prepare job did; each stage's own report, or `{ error }` if it failed. */
export interface PrepareResult {
  started_at_stage: PrepareStage;
  finished: boolean;
  stages: Partial<Record<PrepareStage, Record<string, unknown> & { error?: string }>>;
}

/**
 * Scan, hash, PDF-fingerprint and duplicate-scan the library as one job.
 * Resumable: a rerun continues at the stage the last one reached.
 */
export function prepareLibrary(handlers: LibraryJobHandlers<PrepareResult> = {}): Promise<PrepareResult> {
  return runLibraryJob<PrepareResult>("/prepare-library/default", handlers);
}

/** Which prepare stage a job's current phase belongs to, or null if it is not one of them. */
export function prepareStageOfPhase(phase: string): PrepareStage | null {
  switch (phase) {
    case "walking":
    case "indexing":
    case "enriching":
      return "scan";
    case "hashing":
      return "hash";
    case "fingerprinting":
      return "fingerprint";
    case "matching":
    case "describing":
      return "dedupe";
    default:
      return null;
  }
}

/** "Step 2 of 4 · Reading files… 10 of 50" for the prepare job's progress. */
export function describePrepareProgress(p: LibraryJobProgress): string {
  const stage = prepareStageOfPhase(p.phase);
  const line = describeJobProgress(p);
  if (stage === null) return line;
  return `Step ${PREPARE_STAGES.indexOf(stage) + 1} of ${PREPARE_STAGES.length} · ${line}`;
}

/** One line for the finished prepare job: what it read, and which stages failed. */
export function describePrepareResult(r: PrepareResult, cancelled = false): string {
  const parts: string[] = [];
  if (cancelled || !r.finished) parts.push("stopped early — run it again to continue");
  const failed = PREPARE_STAGES.filter((s) => r.stages[s]?.error);
  for (const s of failed) parts.push(`${s} failed: ${r.stages[s]?.error}`);
  const hash = r.stages.hash as { hashed?: number; total?: number } | undefined;
  if (hash && typeof hash.total === "number") {
    parts.push(hash.total === 0 ? "every file already had a hash" : `${(hash.hashed ?? 0).toLocaleString("en-US")} of ${hash.total.toLocaleString("en-US")} files hashed`);
  }
  const fp = r.stages.fingerprint as { fingerprinted?: number; failed_count?: number } | undefined;
  if (fp && typeof fp.fingerprinted === "number" && fp.fingerprinted > 0) parts.push(`${fp.fingerprinted.toLocaleString("en-US")} PDFs analysed`);
  if (fp && (fp.failed_count ?? 0) > 0) parts.push(`${fp.failed_count} PDFs could not be read`);
  return parts.length ? parts.join(" · ") : "Ready for duplicate detection.";
}

/** One line for a job's progress, for the UI to show while it runs. */
export function describeJobProgress(p: LibraryJobProgress): string {
  const n = (v: number) => v.toLocaleString("en-US");
  switch (p.phase) {
    case "walking":
      return `Looking through the folder… ${n(p.seen)} files so far`;
    case "indexing":
      return `Adding books… ${n(p.added + p.errors)} of ${n(p.total)} new files`;
    case "enriching":
      return `Reading titles and covers… ${n(p.enriched + p.errors)} of ${n(p.total)}`;
    case "hashing":
      return `Reading files… ${n(p.hashed + p.errors)} of ${n(p.total)}`;
    case "fingerprinting":
      return `Analysing PDF pages… ${n((p.fingerprinted ?? 0) + p.errors)} of ${n(p.total)}`;
    case "importing":
      return `Importing… ${n(p.hashed + p.already_known + p.errors)} of ${n(p.total)} files · ${n(p.added)} books added${p.ignored ? ` · ${n(p.ignored)} duplicates skipped` : ""}`;
    case "checking":
    case "columns":
      return "Checking the calibre library…";
    case "absorbing":
      return `Bringing in books… ${n(p.added + p.already_known + p.errors)} of ${n(p.total)}${p.already_known ? ` (${n(p.already_known)} already in)` : ""}`;
    case "finishing":
      return "Finishing…";
    case "removing":
      return `Removing books… ${n(p.added)} of ${n(p.total)}`;
    case "copying":
      return `Copying… ${n(p.added + p.already_known + p.errors)} of ${n(p.total)} books${p.already_known ? ` · ${n(p.already_known)} duplicates skipped` : ""}`;
    case "matching":
      return "Comparing every book in the library…";
    case "describing":
      return `Preparing groups… ${n(p.seen)} of ${n(p.total)}`;
    case "retitling":
      return `Fixing titles… ${n(p.seen)} of ${n(p.total)} books`;
    case "resolving":
      return `Resolving… ${n(p.seen)} of ${n(p.total)} groups`;
    default:
      return "Working…";
  }
}

/** Copies a folder tree into the library, as a job: see `importFolder.ts`. */
export function importFolder(request: ImportFolderRequest, handlers: LibraryJobHandlers<ImportReport> = {}): Promise<ImportReport> {
  return runLibraryJob<ImportReport>("/import-folder/-", handlers, importBody(request));
}

/** The calibre libraries found inside this library (a bounded walk; reads no book). */
export function fetchAbsorbable(): Promise<AbsorbableList> {
  return jsonFetch<AbsorbableList>("/absorbable-libraries/-");
}

/** Registers a nested calibre library's books in place, as a job: see `absorb.ts`. */
export function absorbLibrary(path: string, handlers: LibraryJobHandlers<AbsorbReport> = {}): Promise<AbsorbReport> {
  return runLibraryJob<AbsorbReport>("/absorb-library/-", handlers, { path });
}

/** Undoes an absorb: puts the nested metadata.db back and removes the books (database rows only). */
export function unabsorbLibrary(path: string, handlers: LibraryJobHandlers<UnabsorbReport> = {}): Promise<UnabsorbReport> {
  return runLibraryJob<UnabsorbReport>("/unabsorb-library/-", handlers, { path });
}

export interface UnabsorbReport {
  removed: number;
  database_restored: boolean;
  cancelled: boolean;
}

/** How many book files in the library folder nothing tracks yet. A walk; reads nothing, adds nothing. */
export function fetchUntracked(max = 10000): Promise<UntrackedCount> {
  return jsonFetch<UntrackedCount>(`/library-untracked/-?max=${max}`);
}

/** The library's trash (calibre's `.caltrash`). Also expires what is old enough. */
export function fetchTrash(): Promise<TrashListing> {
  return jsonFetch<TrashListing>("/trash/-");
}

function postEntry<T>(action: string, body: RestoreRequest): Promise<T> {
  return jsonFetch<T>(`/trash/${action}/-`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
}

export interface RestoreResult {
  book_id: number;
  title?: string;
  format?: string;
  path?: string;
  restored?: string[];
  /** `[original, where it went]` for files whose name had been taken. */
  renamed?: [string, string][];
}

export function restoreTrashEntry(request: RestoreRequest): Promise<RestoreResult> {
  return postEntry<RestoreResult>("restore", request);
}

/** For ever. The caller has asked the user first. */
export function deleteTrashEntry(request: RestoreRequest): Promise<void> {
  return postEntry<void>("delete", request);
}

/** For ever. The caller has asked the user first. */
export function emptyTrash(): Promise<{ removed: number }> {
  return jsonFetch<{ removed: number }>("/trash/empty/-", { method: "POST" });
}

/** A book entry whose file is gone — renamed, moved out, or deleted. */
export interface Orphan {
  book_id: number;
  format: string;
  /** Where it was last seen. Often all somebody needs to find it again. */
  last_known_path: string;
  noticed_at: string;
}

/**
 * Whether the file the user picked is the one that was lost.
 *
 * `content_differs` is the case that must reach the user: they may be
 * supplying a re-downloaded copy, or they may have picked the wrong
 * file, and only they can tell which.
 */
export interface RelocateResult {
  outcome: "content_matches" | "content_differs" | "nothing_to_compare";
  message: string;
}

/** A file removed from the library but kept on disk. */
export interface IgnoredFile {
  path: string;
  title: string;
  ignored_at: string;
}

export function fetchOrphans(): Promise<Orphan[]> {
  return jsonFetch<Orphan[]>("/orphans/default");
}

/** Points an orphan at a file already inside the library. */
export function relocateOrphan(bookId: number, format: string, path: string): Promise<RelocateResult> {
  return jsonFetch<RelocateResult>(`/orphans/relocate/${bookId}/${encodeURIComponent(format)}/default`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

/**
 * Copies a file back into the library and points the orphan at it.
 *
 * The `File` goes straight into the body, the same way `addBook` and
 * `setCover` send theirs — `fetch` streams it, so nothing reads the
 * whole thing into a buffer first.
 */
export function uploadOrphanFile(bookId: number, format: string, file: File): Promise<RelocateResult> {
  return jsonFetch<RelocateResult>(`/orphans/upload/${bookId}/${encodeURIComponent(format)}/default/${encodeURIComponent(file.name)}`, { method: "POST", body: file });
}

/**
 * Drops the entry. `keepMetadataAs` writes the book's metadata to that
 * filename in the library first — tags, ratings and reading progress
 * cannot be re-typed the way a book can be re-downloaded.
 */
export function forgetOrphan(bookId: number, keepMetadataAs?: string): Promise<{ forgotten: number; metadata_saved_as: string | null }> {
  const query = keepMetadataAs ? `?keep_metadata=${encodeURIComponent(keepMetadataAs)}` : "";
  return jsonFetch<{ forgotten: number; metadata_saved_as: string | null }>(`/orphans/forget/${bookId}/default${query}`, { method: "POST" });
}

export function fetchIgnored(): Promise<IgnoredFile[]> {
  return jsonFetch<IgnoredFile[]>("/ignored/default");
}

/** Lets the scanner pick a file up again. Does not add it — the next scan does. */
export function unignoreFile(path: string): Promise<void> {
  return jsonFetch<void>("/ignored/unignore/default", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ path }),
  });
}

// Real, new route -- see crates/lexmechanic_srv/src/save_to_disk.rs's own
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

// Real, new route -- see crates/lexmechanic_srv/src/duplicates.rs's own
// doc (real upstream's "Find duplicates" is a Qt GUI action, never
// exposed over HTTP there).
export interface DuplicateBook {
  book_id: number;
  title: string;
  authors: string[];
  /** How many formats the book has. */
  formats?: number;
  /** How many of authors, series, publisher, tags, comments, rating, identifiers, languages are filled in. */
  metadata?: number;
  has_cover?: boolean;
  /** Present for a fingerprinted PDF: what distinguishes a scan from its OCR. */
  pdf?: { pages: number; has_text: boolean; scanned?: boolean; image_only?: boolean; size: number } | null;
  /** The formats it has, upper case. */
  format_list?: string[];
  uuid?: string;
  /** Every format file: its path relative to the library, size in bytes, and modified time (unix seconds). */
  files?: DuplicateFile[];
  /** The book's folder, relative to the library. */
  path?: string;
  /** Bytes of all its formats. */
  size?: number;
  /** The newest modified time among its files (unix seconds). */
  mtime?: number | null;
  /** A thumbnail URL, when the book has a cover. */
  cover_url?: string | null;
}

export interface DuplicateFile {
  format: string;
  path: string;
  size: number;
  mtime: number | null;
}

/**
 * Why a group was reported, strongest first.
 *
 * `content`: a byte-identical file (#892), needing no metadata at all.
 * `identifier`: an equal, valid ISBN (10 and 13 compared as 13) or other
 * strong identifier. `title_author_exact`: equal after normalisation (case,
 * diacritics, articles, `(z-lib.org)` noise, "Last, First", initials).
 * `title_author_fuzzy`: probably the same book, but a person has to say so.
 * See crates/lexmechanic_db/src/dedupe.rs.
 */
export type DuplicateReason =
  | "content"
  | "identifier"
  | "title_author_exact"
  | "title_author_fuzzy"
  | "pdf_resave"
  | "pdf_ocr_variant"
  | "pdf_text_match"
  /** Books the user grouped themselves (tranche 16). */
  | "manual";

/** Where the library's PDFs stand for page-content matching. */
export interface PdfScanStatus {
  fingerprinted: number;
  /** Could not be read (encrypted, corrupt); recorded, not retried. */
  failed: number;
  /** Have a content hash and no fingerprint: what "Fingerprint PDFs" would do. */
  pending: number;
  /** Have no content hash yet. */
  unhashed: number;
}

/** One reason two books in a group were joined. */
export interface DuplicateEvidence {
  books: [number, number];
  reason: DuplicateReason;
  /** 0 to 1. */
  confidence: number;
  detail: string;
}

export interface DuplicateGroup {
  books: DuplicateBook[];
  reason: DuplicateReason;
  /** 0 to 1: the highest confidence among the group's evidence. */
  confidence?: number;
  evidence?: DuplicateEvidence[];
  /** The record the server would merge the others into; see `defaultMergeDestination`. */
  suggested_dest?: number | null;
  /** Stable across rescans: a hash of the members' uuids. */
  id?: string;
  /** How many books the group holds. */
  size?: number;
  /** Why the server suggests that copy to keep. */
  keeper_reason?: string;
  /** Bytes the other copies hold. */
  bytes_reclaimable?: number;
  /** The user's own group this is (or contains), if any. */
  manual_group_id?: string | null;
}

/** A zero-byte book file. Never grouped as a duplicate of another empty file; listed on its own. */
export interface EmptyFile {
  book_id: number;
  format: string;
  title: string;
  authors: string[];
}

/** `min` is applied by the server (weaker evidence is not used at all). Omit for everything. */
export function scanForDuplicates(min?: "content" | "identifier" | "exact" | "pdf" | "fuzzy"): Promise<{ groups: DuplicateGroup[]; pdf?: PdfScanStatus; pdf_unfingerprinted?: number; empty_files?: EmptyFile[] }> {
  const query = min ? `?min=${min}` : "";
  return jsonFetch<{ groups: DuplicateGroup[]; pdf?: PdfScanStatus; pdf_unfingerprinted?: number; empty_files?: EmptyFile[] }>(`/duplicates/scan/default${query}`, { method: "POST" });
}

// ---- Reviewing and resolving duplicates at scale (crates/lexmechanic_srv/src/duplicates_review.rs) ----

export type DuplicateSort = "confidence" | "size" | "reason" | "reclaimable";

export interface DuplicatesQuery {
  offset?: number;
  limit?: number;
  reasons?: DuplicateReason[];
  minConfidence?: number;
  maxConfidence?: number;
  /** Prefix of a file or folder path under the library. */
  path?: string;
  sort?: DuplicateSort;
  order?: "asc" | "desc";
}

/** One page of the server's last scan. */
export interface DuplicatePage {
  has_scan: true;
  /** The library changed since the scan (other than through this panel). */
  stale: boolean;
  generation: number;
  scanned_at: number;
  /** Groups after the filters. */
  total: number;
  /** Groups in the scan. */
  all_total: number;
  offset: number;
  limit: number;
  groups: DuplicateGroup[];
  /** Groups per reason under every filter except the reason filter. */
  facets: { reason: Partial<Record<DuplicateReason, number>> };
  pdf?: PdfScanStatus;
  pdf_unfingerprinted?: number;
  empty_files?: EmptyFile[];
  exemptions_applied?: number;
}

export type DuplicatePageOrNone = DuplicatePage | { has_scan: false };

export function duplicatesQueryString(q: DuplicatesQuery): string {
  const p = new URLSearchParams();
  if (q.offset) p.set("offset", String(q.offset));
  if (q.limit) p.set("limit", String(q.limit));
  if (q.reasons && q.reasons.length > 0) p.set("reason", q.reasons.join(","));
  if (q.minConfidence !== undefined && q.minConfidence > 0) p.set("min_confidence", String(q.minConfidence));
  if (q.maxConfidence !== undefined && q.maxConfidence < 1) p.set("max_confidence", String(q.maxConfidence));
  if (q.path && q.path.trim()) p.set("path", q.path.trim());
  if (q.sort && q.sort !== "confidence") p.set("sort", q.sort);
  if (q.order) p.set("order", q.order);
  const s = p.toString();
  return s ? `?${s}` : "";
}

/** A page of the last scan, or `{has_scan: false}` if the library has not been scanned this session. */
export function fetchDuplicateGroups(q: DuplicatesQuery = {}): Promise<DuplicatePageOrNone> {
  return jsonFetch<DuplicatePageOrNone>(`/duplicates/groups/default${duplicatesQueryString(q)}`);
}

/** What the scan job reports: counts, no groups (those are paged). */
export interface DuplicateScanSummary {
  generation: number;
  group_count: number;
  facets: { reason: Partial<Record<DuplicateReason, number>> };
  pdf?: PdfScanStatus;
  pdf_unfingerprinted?: number;
  empty_files?: EmptyFile[];
  exemptions_applied?: number;
}

/** Scans the whole library as a cancellable job with progress; the groups are then paged with `fetchDuplicateGroups`. */
export function runDuplicateScan(min: DuplicateMinParam, handlers: LibraryJobHandlers<DuplicateScanSummary> = {}): Promise<DuplicateScanSummary> {
  return runLibraryJob<DuplicateScanSummary>(`/duplicates/scan/default?job=true&min=${min}`, handlers);
}

export type DuplicateMinParam = "content" | "identifier" | "exact" | "pdf" | "fuzzy";

/** Forgets groups the UI has dealt with (after a merge or delete done through other routes). `bookIds` are books deleted from groups. */
export function dropDuplicateGroups(groupIds: string[], bookIds: number[] = []): Promise<{ dropped: number; remaining: number | null; stale: boolean | null }> {
  return jsonFetch("/duplicates/drop/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ group_ids: groupIds, book_ids: bookIds }) });
}

export interface DuplicateExemption {
  id: string;
  created_at: number;
  members: { uuid: string; title: string; authors: string[]; book_id: number | null; present: boolean }[];
}

export interface ExemptResult {
  exemption: DuplicateExemption;
  pairs: number;
  dropped_group_ids: string[];
  /** A group overlapped the exempted books without being inside them: rescan to re-split it. */
  needs_rescan: boolean;
}

/** "These are not duplicates": every pair among the group's books (or `bookIds`) is exempt from now on. */
export function markNotDuplicates(target: { groupId?: string; bookIds?: number[] }): Promise<ExemptResult> {
  return jsonFetch<ExemptResult>("/duplicates/exempt/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ group_id: target.groupId, book_ids: target.bookIds ?? [] }) });
}

export function fetchExemptions(): Promise<{ exemptions: DuplicateExemption[]; count: number }> {
  return jsonFetch("/duplicates/exemptions/default");
}

/** Undoes one dismissal (`{id}`) or all of them (`{all: true}`). The groups come back with the next scan. */
export function removeExemption(target: { id: string } | { all: true }): Promise<{ removed: number; needs_rescan: boolean }> {
  return jsonFetch("/duplicates/exemptions/remove/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(target) });
}

export type ResolveAction = "keep_best_trash_rest" | "merge_as_formats";

export interface ResolveSelection {
  action: ResolveAction;
  /** Reasons to include. The server defaults to `["content"]`; title tiers are only ever included when listed. */
  tiers?: DuplicateReason[];
  minConfidence?: number;
  pathPrefix?: string;
  /** Explicit groups; overrides tiers, confidence and path. */
  groupIds?: string[];
  /** `{group id: book id}` keeper overrides. */
  keepers?: Record<string, number>;
  keepDuplicateFormats?: boolean;
}

export interface ResolvePlanGroup {
  group_id: string;
  reason: DuplicateReason;
  confidence: number;
  action: ResolveAction;
  keeper: DuplicateBook;
  keeper_reason: string;
  others: DuplicateBook[];
  bytes_freed: number;
}

export interface ResolvePlan {
  dry_run: true;
  action: ResolveAction;
  plan_fingerprint: string;
  groups: ResolvePlanGroup[];
  totals: { groups: number; books: number; bytes_freed: number };
  skipped: { group_id: string; reason: string }[];
  missing_group_ids: string[];
  /** False for a merge: restoring the run does not undo what was merged into the keeper. */
  fully_restorable: boolean;
  note: string;
}

export interface ResolveReport {
  run_id: string;
  action: ResolveAction;
  groups_planned: number;
  groups_done: number;
  done_groups: string[];
  books_trashed: number;
  bytes_freed: number;
  trashed_files: string[];
  errors: [string, string][];
  cancelled: boolean;
  fully_restorable: boolean;
}

function resolveBody(sel: ResolveSelection, extra: Record<string, unknown>): string {
  return JSON.stringify({
    action: sel.action,
    ...(sel.tiers ? { tiers: sel.tiers } : {}),
    ...(sel.minConfidence !== undefined ? { min_confidence: sel.minConfidence } : {}),
    ...(sel.pathPrefix && sel.pathPrefix.trim() ? { path_prefix: sel.pathPrefix.trim() } : {}),
    ...(sel.groupIds ? { group_ids: sel.groupIds } : {}),
    ...(sel.keepers && Object.keys(sel.keepers).length > 0 ? { keepers: sel.keepers } : {}),
    keep_duplicate_formats: sel.keepDuplicateFormats ?? false,
    ...extra,
  });
}

/** The mandatory dry run: what resolving would do. Changes nothing. */
export function planResolve(sel: ResolveSelection): Promise<ResolvePlan> {
  return jsonFetch<ResolvePlan>("/duplicates/resolve/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: resolveBody(sel, { dry_run: true }) });
}

/**
 * Executes the plan the user confirmed (`planFingerprint`), as a cancellable job.
 * The server refuses (409) if the library or the selection changed since the preview.
 */
export function runResolve(sel: ResolveSelection, planFingerprint: string, handlers: LibraryJobHandlers<ResolveReport> = {}): Promise<ResolveReport> {
  return runLibraryJob<ResolveReport>("/duplicates/resolve/default", handlers, JSON.parse(resolveBody(sel, { dry_run: false, plan_fingerprint: planFingerprint })));
}

export interface ResolveRun {
  run_id: string;
  books: number;
  bytes: number;
  deleted_at: number;
}

/** Runs that still have books in the library trash. */
export function fetchResolveRuns(): Promise<{ runs: ResolveRun[] }> {
  return jsonFetch("/duplicates/runs/default");
}

/** Puts back every book the run trashed. */
export function restoreResolveRun(runId: string): Promise<{ run_id: string; restored: { book_id: number; title: string }[]; failed: [string, string][]; needs_rescan: boolean }> {
  return jsonFetch(`/duplicates/runs/default/${encodeURIComponent(runId)}/restore`, { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" });
}

// ---- Comparing duplicates, the user's groups, moving books (crates/lexmechanic_srv/src/duplicates_compare.rs) ----

export interface ComparePageMatch {
  /** 0-based sample pages of the first and second book. */
  a: number;
  b: number;
  /** Of 256: how different the two pages look. */
  distance: number;
  matched: boolean;
}

export interface ComparePdf {
  /** False until both PDFs have been analysed (not an error). */
  fingerprinted: boolean;
  missing?: number[];
  /** The `pdf_*` reason the library scan would give the pair, or null if it is not a match. */
  kind?: DuplicateReason | null;
  /** Graded within the tier's band. */
  confidence?: number | null;
  detail?: string | null;
  visual_match?: boolean;
  compared?: number;
  matched?: number;
  mean_distance?: number;
  pages?: ComparePageMatch[];
  text_jaccard?: number | null;
  page_counts?: [number, number];
  has_text?: [boolean, boolean];
}

export interface ComparePair {
  a: number;
  b: number;
  /** True when a format of each has the same content; null when a file has no hash yet. */
  identical_bytes: boolean | null;
  identical_formats: string[];
  /** The user said these two are not duplicates. */
  exempt: boolean;
  /** Null when either book has no PDF. */
  pdf: ComparePdf | null;
  evidence: { reason: DuplicateReason; confidence: number; detail: string }[];
}

export interface CompareBook extends DuplicateBook {
  suggested?: boolean;
}

export interface CompareResult {
  ids: number[];
  books: CompareBook[];
  pairs: ComparePair[];
  suggested_keeper: number | null;
  keeper_reason: string | null;
  pdf: { books: number; fingerprinted: number };
}

/** 2 to 8 books, any books, from the library as it is now. */
export function fetchCompare(ids: number[]): Promise<CompareResult> {
  return jsonFetch<CompareResult>(`/duplicates/compare/default?ids=${ids.join(",")}`);
}

export interface ManualGroup {
  id: string;
  created_at: number;
  members: { uuid: string; title: string; authors: string[]; book_id: number | null; present: boolean }[];
  book_ids: number[];
  /** The review snapshot's group it is part of, when there is a scan. */
  snapshot_group_id?: string | null;
}

export interface ManualGroupResult {
  manual_group: ManualGroup | null;
  /** The review snapshot's group (with any detected group it overlaps folded in); null without a scan. */
  group: DuplicateGroup | null;
  /** The group had fewer than two books left and is gone. */
  removed: boolean;
}

const JSON_POST = { method: "POST", headers: { "Content-Type": "application/json" } } as const;

/** The user's groups. */
export function fetchManualGroups(): Promise<{ count: number; groups: ManualGroup[] }> {
  return jsonFetch("/duplicates/manual-groups/default");
}

/** Makes a group of the books (at least two). Books already in a group of the user's bring it along. */
export function createManualGroup(bookIds: number[]): Promise<ManualGroupResult> {
  return jsonFetch<ManualGroupResult>("/duplicates/manual-groups/default", { ...JSON_POST, body: JSON.stringify({ book_ids: bookIds }) });
}

export function addToManualGroup(groupId: string, bookIds: number[]): Promise<ManualGroupResult> {
  return jsonFetch<ManualGroupResult>("/duplicates/manual-groups/default", { ...JSON_POST, body: JSON.stringify({ group_id: groupId, add: bookIds }) });
}

export function removeFromManualGroup(groupId: string, bookIds: number[]): Promise<ManualGroupResult> {
  return jsonFetch<ManualGroupResult>("/duplicates/manual-groups/default", { ...JSON_POST, body: JSON.stringify({ group_id: groupId, remove: bookIds }) });
}

export function deleteManualGroup(groupId: string): Promise<{ removed: boolean; group_id: string }> {
  return jsonFetch("/duplicates/manual-groups/remove/default", { ...JSON_POST, body: JSON.stringify({ group_id: groupId }) });
}

/** Tells the review list that these books were changed by the caller itself (title, file name, folder), so a resolve is not refused for it. */
export function refreshDuplicateBooks(bookIds: number[]): Promise<{ refreshed: number; has_scan: boolean; stale?: boolean | null }> {
  return jsonFetch("/duplicates/refresh-books/default", { ...JSON_POST, body: JSON.stringify({ book_ids: bookIds }) });
}

export type MovePlan = OrganizePlan & { folder: string };

/** The dry run of moving books to a folder (relative to the library). Changes nothing. */
export function planMoveBooks(bookIds: number[], folder: string): Promise<MovePlan> {
  return jsonFetch<MovePlan>("/move-books/default", { ...JSON_POST, body: JSON.stringify({ book_ids: bookIds, folder, dry_run: true }) });
}

/** Moves the previewed books, as a job; the run is undone with `undoOrganize`. */
export function runMoveBooks(bookIds: number[], folder: string, planFingerprint: string, handlers: LibraryJobHandlers<OrganizeReport> = {}): Promise<OrganizeReport> {
  return runLibraryJob<OrganizeReport>("/move-books/default", handlers, { book_ids: bookIds, folder, dry_run: false, plan_fingerprint: planFingerprint });
}

/** Which books to organize: exactly one of the three, or none for every book. */
export interface OrganizeScope {
  bookIds?: number[];
  search?: string;
  pathPrefix?: string;
}

export interface OrganizeMove {
  book_id: number;
  title: string;
  format: string;
  from: string;
  to: string;
  /** The name had to be numbered, because the target was taken. */
  suffixed: boolean;
}

export interface OrganizePlan {
  plan_fingerprint: string;
  template: string;
  books: number;
  files: number;
  /** Books whose target name was taken and so got a number. */
  conflicts: number;
  already_organized: number;
  skipped: { book_id: number; reason: string }[];
  /** At most 500; `books` and `files` are the totals. */
  moves: OrganizeMove[];
}

export interface OrganizeReport {
  run_id: string;
  books_planned: number;
  books_moved: number;
  files_moved: number;
  failed: { book_id: number; reason: string }[];
  cancelled: boolean;
}

export interface OrganizeUndoReport {
  run_id: string;
  books_restored: number;
  files_restored: number;
  skipped: { book_id: number; reason: string }[];
}

export interface OrganizeRun {
  run_id: string;
  /** Seconds since the epoch. */
  created_at: number;
  books: number;
  undone: boolean;
  /** What the run was asked to do: a template, `move to Fiction` or `rename folder A to B`. Empty on an older server. */
  template?: string;
}

function organizeBody(template: string, scope: OrganizeScope, extra: Record<string, unknown>): string {
  return JSON.stringify({
    ...(template.trim() ? { template: template.trim() } : {}),
    ...(scope.bookIds ? { book_ids: scope.bookIds } : {}),
    ...(scope.search !== undefined ? { search: scope.search } : {}),
    ...(scope.pathPrefix !== undefined ? { path_prefix: scope.pathPrefix } : {}),
    ...extra,
  });
}

/** The dry run: what organizing would move. Changes nothing. */
export function planOrganize(template: string, scope: OrganizeScope): Promise<OrganizePlan> {
  return jsonFetch<OrganizePlan>("/organize-library/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: organizeBody(template, scope, { dry_run: true }) });
}

/** Executes the previewed plan as a job. The server answers 409 if the library or scope changed since. */
export function runOrganize(template: string, scope: OrganizeScope, planFingerprint: string, handlers: LibraryJobHandlers<OrganizeReport> = {}): Promise<OrganizeReport> {
  return runLibraryJob<OrganizeReport>("/organize-library/default", handlers, JSON.parse(organizeBody(template, scope, { dry_run: false, plan_fingerprint: planFingerprint })));
}

/** Runs that can be undone, newest first as the server lists them. */
export function fetchOrganizeRuns(): Promise<{ runs: OrganizeRun[] }> {
  return jsonFetch("/organize-runs/default");
}

/** Moves a run's files back, as a job. */
export function undoOrganize(runId: string, handlers: LibraryJobHandlers<OrganizeUndoReport> = {}): Promise<OrganizeUndoReport> {
  return runLibraryJob<OrganizeUndoReport>("/unorganize-library/default", handlers, { run_id: runId });
}

// ----- Fix titles (`POST /retitle-library`) -----------------------------

/** Which books to look at: the listed ones (all returned), or every book (only those that need fixing). */
export type RetitleScope = { bookIds: number[] } | { all: true };

export type RetitleReason = "junk" | "template_leftover" | "ok" | "no_suggestion";

export interface RetitleRow {
  book_id: number;
  current_title: string;
  /** Empty for `no_suggestion`: the user types one. */
  suggested_title: string;
  reason: RetitleReason;
  /** Whether the suggestion reads like a title rather than a catalogue number; only these start selected. */
  confident: boolean;
  /** The file stem(s) now. */
  current_file: string;
  formats: string[];
  /** The stem the rename template gives with the suggested title; null when there is none to show. */
  proposed_file: string | null;
}

export interface RetitleSuggestions {
  rename_template: string;
  /** Books needing a fix in the scope, whether or not they fit in `rows`. */
  total: number;
  /** `rows` is shorter than `total` (the server caps at 5000). */
  truncated: boolean;
  rows: RetitleRow[];
}

export interface RetitleItem {
  book_id: number;
  /** The title the user saw: the server skips the book if it holds another now. */
  expected_title: string;
  title: string;
}

export interface RetitleOptions {
  embed: boolean;
  renameFiles: boolean;
  renameTemplate: string;
}

export interface RetitleReport {
  run_id: string;
  books_planned: number;
  retitled: number;
  embedded: number;
  renamed: number;
  /** `reason`: `changed_since_preview`, `invalid` or `no_such_book`. */
  skipped: { book_id: number; reason: string }[];
  /** `stage`: `title`, `embed` or `rename`; `title_set` says the new title is stored anyway. */
  failed: { book_id: number; stage: string; reason: string; title_set: boolean }[];
  cancelled: boolean;
}

export interface RetitleUndoReport {
  run_id: string;
  titles_restored: number;
  files_restored: number;
  re_embedded: number;
  skipped: { book_id: number; reason: string }[];
  failed: { book_id: number; stage: string; reason: string; title_set: boolean }[];
  cancelled: boolean;
}

export interface RetitleRun {
  run_id: string;
  /** Seconds since the epoch. */
  created_at: number;
  books: number;
  undone: boolean;
}

/** The dry run: titles that need fixing, with a suggestion each. Changes nothing. */
export function suggestRetitles(scope: RetitleScope, renameTemplate: string): Promise<RetitleSuggestions> {
  const body = { dry_run: true, scope: "all" in scope ? { all: true } : { book_ids: scope.bookIds }, ...(renameTemplate.trim() ? { rename_template: renameTemplate.trim() } : {}) };
  return jsonFetch<RetitleSuggestions>("/retitle-library/default", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
}

/** Applies the approved titles as a job (set, optionally embed, optionally rename). */
export function runRetitle(items: RetitleItem[], options: RetitleOptions, handlers: LibraryJobHandlers<RetitleReport> = {}): Promise<RetitleReport> {
  const body = { dry_run: false, items, embed: options.embed, rename_files: options.renameFiles, ...(options.renameTemplate.trim() ? { rename_template: options.renameTemplate.trim() } : {}) };
  return runLibraryJob<RetitleReport>("/retitle-library/default", handlers, body);
}

/** Recorded runs, newest first. */
export function fetchRetitleRuns(): Promise<{ runs: RetitleRun[] }> {
  return jsonFetch("/retitle-runs/default");
}

/** Puts a run's titles and file names back, as a job. */
export function undoRetitle(runId: string, handlers: LibraryJobHandlers<RetitleUndoReport> = {}): Promise<RetitleUndoReport> {
  return runLibraryJob<RetitleUndoReport>("/unretitle-library/default", handlers, { run_id: runId });
}

/** What the PDF fingerprint job did. */
export interface PdfFingerprintResult {
  /** Distinct file contents fingerprinted. */
  fingerprinted: number;
  total: number;
  /** `[path, reason]` for PDFs that could not be read (encrypted or corrupt). */
  failed: [string, string][];
  failed_count: number;
  missing: string[];
  missing_count: number;
  cancelled: boolean;
}

/**
 * Reads each PDF that has a content hash and no fingerprint, once, so
 * "Find duplicates" can match a scan to its OCR and a PDF to its optimized
 * copy. Cancellable; run again to continue. Rejects if PDFium is not installed.
 */
export function fingerprintPdfs(handlers: LibraryJobHandlers<PdfFingerprintResult> = {}): Promise<PdfFingerprintResult> {
  return runLibraryJob<PdfFingerprintResult>("/pdf-fingerprint/default", handlers);
}

// ---- Merge books (crates/lexmechanic_srv/src/merge_books.rs) ----

export type MergeMode = "merge_and_delete" | "merge_keep_sources" | "formats_only";

export interface MergeRequest {
  dest: number;
  sources: number[];
  mode?: MergeMode;
  keep_duplicate_formats?: boolean;
  dry_run?: boolean;
}

/** What happened (or, for a dry run, would happen) to one source book. */
export interface MergeSourceResult {
  book_id: number;
  title: string;
  formats_moved: string[];
  formats_identical: string[];
  formats_discarded: string[];
  formats_kept_as_data_file: [string, string][];
  formats_left_on_source: string[];
  data_files_moved: string[];
  deleted: boolean;
  trashed: string[];
  failed: [string, string][];
  error: string | null;
  warnings: string[];
}

export interface MergeResponse {
  ok: boolean;
  dry_run: boolean;
  dest: number;
  mode: MergeMode;
  results: MergeSourceResult[];
  metadata_changed: string[];
  cover_from: number | null;
  /** Everything moved to the library trash. */
  trashed: string[];
  failed: { book_id: number; path: string; error: string }[];
  errors: { book_id: number; error: string }[];
  warnings: string[];
}

/** Merges `sources` into `dest`. With `dry_run` nothing is changed. */
export function mergeBooks(req: MergeRequest): Promise<MergeResponse> {
  return jsonFetch<MergeResponse>("/cdb/merge-books/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}

export interface BulkMergePlanBook extends DuplicateBook {
  formats: number;
  metadata: number;
  has_cover: boolean;
}

export interface BulkMergePlan {
  dry_run: true;
  groups: { dest: number; sources: number[]; books: BulkMergePlanBook[] }[];
  group_count: number;
  books_to_merge: number;
  /** Echo this back to execute exactly this plan. */
  plan_fingerprint: string;
}

export interface BulkMergeReport {
  groups_planned: number;
  groups_merged: number;
  books_merged: number;
  trashed: string[];
  errors: [number, string][];
  cancelled: boolean;
}

/** The dry run: what merging every exact-content group would do. Changes nothing. */
export function planBulkMerge(): Promise<BulkMergePlan> {
  return jsonFetch<BulkMergePlan>("/duplicates/merge-exact/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ dry_run: true }),
  });
}

/**
 * Executes the plan the user confirmed, as a cancellable job. The server
 * refuses (409) if the library has changed since `planFingerprint`.
 */
export function runBulkMerge(planFingerprint: string, keepDuplicateFormats: boolean, handlers: LibraryJobHandlers<BulkMergeReport> = {}): Promise<BulkMergeReport> {
  return runLibraryJob<BulkMergeReport>("/duplicates/merge-exact/default", handlers, { dry_run: false, plan_fingerprint: planFingerprint, keep_duplicate_formats: keepDuplicateFormats });
}

// Real, new routes -- see crates/lexmechanic_srv/src/news_scheduler.rs's
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

// Real, new routes -- see crates/lexmechanic_srv/src/metadata_search.rs's
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
// crates/lexmechanic_srv/src/plugins.rs.
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

// Fonts and diff in the editor (#816 items 3.6 / 3.5).
export interface FontFamily {
  family: string;
  embedded: boolean;
}

export function bookFonts(sessionId: string): Promise<{ count: number; embedded: number; not_embedded: number; families: FontFamily[] }> {
  return jsonFetch(`/tweak/fonts/${encodeURIComponent(sessionId)}`);
}

export interface DiffLine {
  tag: "add" | "remove" | "context";
  old_line: number | null;
  new_line: number | null;
  text: string;
}

export interface DiffFile {
  name: string;
  status: "added" | "removed" | "modified" | "binary-changed";
  lines?: DiffLine[];
}

export function bookDiff(sessionId: string): Promise<{ changed: number; files: DiffFile[] }> {
  return jsonFetch(`/tweak/diff/${encodeURIComponent(sessionId)}`);
}

// Persisted email account (#816 item 4.2). Everything but the
// password: the store is a plain JSON blob on the server and
// lexmechanic_srv can be served over a network, so a credential written
// there would be readable by more people than a user configuring an
// email account would expect.
export interface EmailAccount {
  relay: string;
  port?: number | null;
  username?: string | null;
  encryption?: string | null;
  from?: string | null;
}

export async function getEmailAccount(): Promise<EmailAccount | null> {
  const data = await jsonFetch<{ account: EmailAccount | null }>("/email-account");
  return data.account;
}

export function saveEmailAccount(account: EmailAccount): Promise<{ ok: boolean }> {
  return jsonFetch<{ ok: boolean }>("/email-account", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(account),
  });
}

// Bulk template evaluation (#816 item 4.1). Colouring a page of rows
// is one evaluation per row; the per-book route would be a request
// per row.
export async function evaluateTemplateBulk(template: string, bookIds: number[]): Promise<Record<string, string>> {
  const data = await jsonFetch<{ ok: boolean; results?: Record<string, string>; error?: string }>("/template-tester/evaluate-bulk/default", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ template, book_ids: bookIds }),
  });
  // A broken template is an expected outcome while a rule is being
  // written; the caller treats it as "this rule colours nothing".
  return data.ok && data.results ? data.results : {};
}

export interface LibraryInfo {
  /** Display name — the library folder's own name. */
  name: string;
  /** Absolute path on disk, for disambiguating same-named libraries. */
  path: string;
  /** `library_id` of the open library. */
  id: string;
  /**
   * Every library this server hosts, `library_id` to display name (#959).
   *
   * One entry when the server was started with a single library, which is
   * how it reads for anyone who has only ever opened one.
   */
  libraries: Record<string, string>;
  /** Where the library lives and whether another program has it open (#258); null from an older server. */
  storage: StorageInfo | null;
}

/**
 * Which library is open.
 *
 * Until recently `/ajax/library-info` reported a hardcoded name and no
 * path at all, so nothing the UI could show distinguished one library
 * from another.
 */
export async function fetchLibraryInfo(): Promise<LibraryInfo> {
  const res = await fetch("/ajax/library-info");
  if (!res.ok) throw new Error(`library info: ${res.status}`);
  const body = (await res.json()) as { library_map?: Record<string, string>; default_library?: string; library_path?: string; storage?: unknown };
  const id = body.default_library ?? "default";
  const libraries = body.library_map ?? {};
  return { name: libraries[id] ?? "Library", path: body.library_path ?? "", id, libraries, storage: parseStorageInfo(body.storage) };
}

/**
 * Copies books into another library this server hosts (#959, #816 item
 * 1.4).
 *
 * The target is a `library_id` from `LibraryInfo.libraries`, not a path:
 * the server only copies into a library it already has open, so a folder
 * it was not started with is not a valid destination.
 *
 * The request key is `move_books`, which is what the server documents
 * (it also accepts the bare `move`).
 *
 * Returns the per-book outcome the route reports — a book that already
 * exists in the target can be skipped while its neighbours are copied, so
 * a single overall success would be the wrong shape.
 */
export interface CopyOutcome {
  ok: boolean;
  /** The new book's id when `ok`; the reason (a string) when not; null for a skipped duplicate. */
  payload?: number | string | null;
  /** Kept for older servers that reported the reason here. */
  error?: string;
  /** Custom columns that were not copied: the target has no same-named, same-typed column. */
  skipped_custom_columns?: string[];
  /** The copy worked but something about the source cleanup did not. */
  warnings?: string[];
}

/** A one-line description of what happened to one book. */
export function describeCopyOutcome(bookId: string, outcome: CopyOutcome): string {
  if (!outcome.ok) {
    const why = typeof outcome.payload === "string" ? outcome.payload : (outcome.error ?? "not copied");
    return `Book ${bookId}: ${why}`;
  }
  if (outcome.payload === null || outcome.payload === undefined) return `Book ${bookId}: skipped (already in the target)`;
  const notes = [...(outcome.warnings ?? [])];
  if (outcome.skipped_custom_columns?.length) notes.push(`custom columns not copied: ${outcome.skipped_custom_columns.join(", ")}`);
  return notes.length ? `Book ${bookId}: copied (${notes.join("; ")})` : `Book ${bookId}: copied`;
}

export async function copyToLibrary(targetLibraryId: string, bookIds: number[], moveBooks = false): Promise<Record<string, CopyOutcome>> {
  if (bookIds.length === 0) return {};
  return await jsonFetch(`/cdb/copy-to-library/${encodeURIComponent(targetLibraryId)}/-`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ book_ids: bookIds, move_books: moveBooks, duplicate_action: "add" }),
  });
}

// ----- Export a copy (`POST /mirror-library`) ----------------------------

export type MirrorMode = "travel" | "mirror";

/** Which books a travel copy holds: one of `book_ids` / `folder_prefix`, or everything. */
export interface MirrorScope {
  all?: boolean;
  book_ids?: number[];
  folder_prefix?: string;
}

export interface MirrorRequest {
  target: string;
  mode: MirrorMode;
  scope?: MirrorScope;
  remove_missing: boolean;
  overwrite_changed: boolean;
}

export interface MirrorCount {
  files: number;
  bytes: number;
}

export interface MirrorKindTotals {
  copy_new: MirrorCount;
  update: MirrorCount;
  unchanged: MirrorCount;
  remove: MirrorCount;
  conflict: MirrorCount;
}

export interface MirrorTotals extends MirrorKindTotals {
  bytes_to_copy: number;
  by_kind: Record<string, MirrorKindTotals>;
}

export interface MirrorPlanItem {
  path: string;
  kind: string;
  bytes: number;
  reason: string | null;
}

export type MirrorAction = "copy_new" | "update" | "remove" | "conflict";

export interface MirrorPlan {
  fingerprint: string;
  mode: MirrorMode;
  target: string;
  new_target: boolean;
  /** `local_internal` | `local_external` | `network`. */
  tier: string;
  network: boolean;
  books: number;
  scope: string;
  totals: MirrorTotals;
  /** At most 500 per action; `totals` are complete. */
  items: Record<MirrorAction, MirrorPlanItem[]>;
  truncated: Record<MirrorAction, boolean>;
  /** Travel: change-log entries made on the copy itself. */
  copy_edits: { count: number; paths: string[] };
  /** Files the database lists that are not on disk. */
  missing_sources: { count: number; paths: string[] };
}

export interface MirrorProblem {
  path: string;
  message: string;
}

export interface MirrorReport {
  run_id: string;
  mode: MirrorMode;
  target: string;
  copied: number;
  updated: number;
  unchanged: number;
  removed: number;
  bytes_copied: number;
  conflicts: MirrorProblem[];
  failures: MirrorProblem[];
  cancelled: boolean;
  edits_set_aside: number;
  trash: string | null;
  complete: boolean;
}

export interface MirrorTarget {
  target: string;
  mode: MirrorMode;
  /** As the server stored it: `"all"`, `{ book_ids }`, `{ folder_prefix }` or `{ too_large }`. */
  scope: "all" | { book_ids: number[] } | { folder_prefix: string } | { too_large: number } | null;
  remove_missing: boolean;
  overwrite_changed: boolean;
  /** Seconds since the epoch. */
  last_run_at: number;
  last_run_id: string | null;
  last_result: { complete: boolean; cancelled?: boolean; copied?: number; updated?: number; unchanged?: number; removed?: number; conflicts?: number; failures?: number; bytes_copied?: number; error?: string } | null;
  /** Whether the folder answers right now. */
  reachable: boolean;
}

/** The dry run: what the copy would do. Changes nothing. */
export function planMirror(request: MirrorRequest): Promise<MirrorPlan> {
  return jsonFetch<MirrorPlan>("/mirror-library/default", { ...JSON_POST, body: JSON.stringify({ ...request, dry_run: true }) });
}

/** Runs the previewed copy as a job. The server answers 409 if it is no longer that plan. */
export function runMirror(request: MirrorRequest, fingerprint: string, handlers: LibraryJobHandlers<MirrorReport> = {}): Promise<MirrorReport> {
  return runLibraryJob<MirrorReport>("/mirror-library/default", handlers, { ...request, dry_run: false, fingerprint });
}

export async function fetchMirrorTargets(): Promise<MirrorTarget[]> {
  return (await jsonFetch<{ targets: MirrorTarget[] }>("/mirror-targets/default")).targets;
}

/** Forgets a remembered target. Never deletes a file. */
export async function forgetMirrorTarget(target: string): Promise<MirrorTarget[]> {
  return (await jsonFetch<{ targets: MirrorTarget[] }>("/mirror-targets/remove/default", { ...JSON_POST, body: JSON.stringify({ target }) })).targets;
}

// ----- Folders and the naming convention (tranche 19b) ------------------

export interface LibraryFolder {
  /** `/`-separated, relative to the library. */
  path: string;
  /** Books in this folder and below it. */
  books: number;
  /** Books directly in it. */
  books_here: number;
  /** Made by the user and holding no books yet. */
  empty: boolean;
}

/** The library's folder tree: one pass over the books' folders, counts include sub-folders. */
export function fetchLibraryFolders(): Promise<{ total: number; folders: LibraryFolder[] }> {
  return jsonFetch("/library-folders/default");
}

/** Makes an empty folder (the directory is created and remembered). */
export function createLibraryFolder(path: string): Promise<{ path: string }> {
  return jsonFetch("/library-folders/default", { ...JSON_POST, body: JSON.stringify({ path }) });
}

/** Removes an empty folder. */
export function deleteLibraryFolder(path: string): Promise<{ ok: boolean }> {
  return jsonFetch("/library-folders/delete/default", { ...JSON_POST, body: JSON.stringify({ path }) });
}

export type RenameFolderPlan = OrganizePlan & { from: string; to: string };

/** The dry run of renaming a folder: every book in it moves, sub-folders and file names kept. */
export function planRenameFolder(from: string, to: string): Promise<RenameFolderPlan> {
  return jsonFetch<RenameFolderPlan>("/rename-folder/default", { ...JSON_POST, body: JSON.stringify({ from, to, dry_run: true }) });
}

/** Renames the previewed folder, as a job; undo with `undoOrganize`. */
export function runRenameFolder(from: string, to: string, planFingerprint: string, handlers: LibraryJobHandlers<OrganizeReport> = {}): Promise<OrganizeReport> {
  return runLibraryJob<OrganizeReport>("/rename-folder/default", handlers, { from, to, dry_run: false, plan_fingerprint: planFingerprint });
}

export interface NamingConvention {
  folder_template: string;
  file_template: string;
  /** `folder/file`, the one template organize takes. */
  combined: string;
  defaults: { folder_template: string; file_template: string };
}

export interface NamingPreview {
  book_id: number;
  title: string;
  folder: string | null;
  file: string | null;
  /** `folder/file.ext` as organize would place it. */
  path: string | null;
  error: string | null;
}

export function fetchNaming(): Promise<NamingConvention> {
  return jsonFetch("/naming/default");
}

export function saveNaming(folderTemplate: string, fileTemplate: string): Promise<NamingConvention> {
  return jsonFetch("/naming/default", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ folder_template: folderTemplate, file_template: fileTemplate }) });
}

/** Runs the templates over up to 5 books (those given, else spread over the library). */
export function previewNaming(folderTemplate: string, fileTemplate: string, bookIds?: number[]): Promise<{ previews: NamingPreview[]; combined: string }> {
  return jsonFetch("/naming-preview/default", { ...JSON_POST, body: JSON.stringify({ folder_template: folderTemplate, file_template: fileTemplate, ...(bookIds && bookIds.length > 0 ? { book_ids: bookIds } : {}) }) });
}
