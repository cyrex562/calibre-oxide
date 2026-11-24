// The library list's model: every matching book id, and a lazy cache
// of the summaries that have actually been needed.
//
// Why ids first: an id list for 50,000 books is a few hundred KB and
// one cheap query, whereas 50,000 summaries is a very large response
// and a slow render. So the list always knows its full length (the
// scrollbar is honest, select-all and shift-range work over rows that
// were never loaded) and fetches metadata only for the rows near the
// viewport, in aligned chunks so scrolling back never refetches.

import { shallowReactive, shallowRef, type ShallowRef } from "vue";

import type { BookSummary } from "./types";

export const CHUNK_SIZE = 100;
/** Chunk requests in flight at once; the rest wait in a queue that a scroll replaces. */
export const MAX_IN_FLIGHT = 2;
/**
 * Most summaries kept at once. Scrolling a 60,000-book library used to cache every row it
 * ever showed (the heap grew to the whole library); past this the chunks scrolled
 * furthest back are dropped and fetched again if the user returns to them.
 */
export const MAX_CACHED_ROWS = 5000;

export interface IdList {
  ids: number[];
  total: number;
}

export interface BookWindowOptions {
  /** The ordered ids of every book matching the current query. */
  fetchIds: () => Promise<IdList>;
  /** Summaries for some ids (missing ones may be left out). */
  fetchBooks: (ids: number[]) => Promise<BookSummary[]>;
  chunkSize?: number;
  maxInFlight?: number;
  /** Most rows kept (default [`MAX_CACHED_ROWS`]); the ones around the viewport are never dropped. */
  maxCachedRows?: number;
}

export interface BookWindow {
  ids: ShallowRef<number[]>;
  total: ShallowRef<number>;
  /** True while an id list is being fetched. */
  loading: ShallowRef<boolean>;
  error: ShallowRef<string | null>;
  /** Bumped on every reset (query change): views scroll back to the top. */
  resetCount: ShallowRef<number>;
  /** Bumped whenever a summary arrives or changes, for views that cache derived data. */
  version: ShallowRef<number>;
  cache: Map<number, BookSummary>;
  /** A new query: forget everything, then load the id list. */
  reset(): Promise<void>;
  /** The same query again: new id list, loaded rows stay on screen until replaced. */
  refresh(): Promise<void>;
  /** The rows `[first, last)` are wanted now. Loads the chunks they touch. */
  setRange(first: number, last: number): void;
  idAt(index: number): number | undefined;
  indexOf(id: number): number;
  bookAt(index: number): BookSummary | undefined;
  getById(id: number): BookSummary | undefined;
  /** Ids in `[from, to]` (either order), inclusive, loaded or not. */
  idsBetween(a: number, b: number): number[];
  /**
   * Fetch any of these ids that are not cached, then return their summaries (in order, those
   * that exist). The returned rows are the answer: the cache may not keep them all.
   */
  ensureLoaded(ids: number[]): Promise<BookSummary[]>;
  /** Replace a cached summary (an edit), returning the previous one. */
  patch(id: number, changes: Partial<BookSummary>): BookSummary | undefined;
  put(book: BookSummary): void;
  /** Drop cached rows so they are refetched when next wanted. */
  invalidate(ids: number[]): void;
  /** Count of chunk requests currently running (for tests and the status bar). */
  pendingCount(): number;
}

export function createBookWindow(options: BookWindowOptions): BookWindow {
  const chunkSize = options.chunkSize ?? CHUNK_SIZE;
  const maxInFlight = options.maxInFlight ?? MAX_IN_FLIGHT;
  const maxRows = Math.max(chunkSize * 4, options.maxCachedRows ?? MAX_CACHED_ROWS);

  const ids = shallowRef<number[]>([]);
  const total = shallowRef(0);
  const loading = shallowRef(false);
  const error = shallowRef<string | null>(null);
  const resetCount = shallowRef(0);
  const version = shallowRef(0);
  const cache = shallowReactive(new Map<number, BookSummary>());

  let indexById = new Map<number, number>();
  /** Bumped whenever the id list is replaced, so late chunk results can be recognised as stale. */
  let generation = 0;
  /** Chunks whose summaries are current for this generation. */
  let loadedChunks = new Set<number>();
  const inFlight = new Set<number>();
  let queue: number[] = [];
  let wanted = { first: 0, last: 0 };
  let listToken = 0;
  /** When each chunk was last wanted or loaded, for dropping the least recently used. */
  const lastUsed = new Map<number, number>();
  let tick = 0;

  /**
   * Keeps the cache within `maxRows` by dropping whole chunks (a row dropped from a chunk
   * that still counts as loaded would stay blank for good), least recently used first and
   * never the ones around the viewport.
   */
  function enforceCap() {
    if (cache.size <= maxRows) return;
    const near = new Set(chunksFor(wanted.first - chunkSize, wanted.last + chunkSize));
    const order = [...lastUsed.entries()].filter(([c]) => !near.has(c)).sort((a, b) => a[1] - b[1]);
    for (const [chunk] of order) {
      if (cache.size <= maxRows) break;
      for (const id of ids.value.slice(chunk * chunkSize, (chunk + 1) * chunkSize)) cache.delete(id);
      loadedChunks.delete(chunk);
      lastUsed.delete(chunk);
    }
    if (cache.size <= maxRows) return;
    // Rows that arrived outside any loaded chunk (a selection's ensureLoaded, a put):
    // oldest first, again sparing the viewport.
    for (const id of [...cache.keys()]) {
      if (cache.size <= maxRows) break;
      const at = indexById.get(id);
      if (at !== undefined && near.has(Math.floor(at / chunkSize))) continue;
      cache.delete(id);
      if (at !== undefined) loadedChunks.delete(Math.floor(at / chunkSize));
    }
  }

  function bump() {
    version.value += 1;
  }

  function chunksFor(first: number, last: number): number[] {
    const n = ids.value.length;
    const a = Math.max(0, Math.min(first, n));
    const b = Math.max(a, Math.min(last, n));
    if (b <= a) return [];
    const out: number[] = [];
    for (let c = Math.floor(a / chunkSize); c <= Math.floor((b - 1) / chunkSize); c++) out.push(c);
    return out;
  }

  function pump() {
    while (inFlight.size < maxInFlight && queue.length > 0) {
      const chunk = queue.shift()!;
      if (loadedChunks.has(chunk) || inFlight.has(chunk)) continue;
      void runChunk(chunk);
    }
  }

  async function runChunk(chunk: number) {
    const gen = generation;
    inFlight.add(chunk);
    const slice = ids.value.slice(chunk * chunkSize, (chunk + 1) * chunkSize);
    try {
      const books = await options.fetchBooks(slice);
      // Results for a list that has since been replaced are dropped:
      // ids may now sit at different indexes, and after a reset they
      // may not belong to the list at all.
      if (gen !== generation) return;
      for (const b of books) cache.set(b.id, b);
      loadedChunks.add(chunk);
      lastUsed.set(chunk, ++tick);
      enforceCap();
      bump();
    } catch (e) {
      if (gen === generation) error.value = e instanceof Error ? e.message : String(e);
    } finally {
      inFlight.delete(chunk);
      pump();
    }
  }

  function setRange(first: number, last: number) {
    wanted = { first, last };
    for (const c of chunksFor(first, last)) if (loadedChunks.has(c)) lastUsed.set(c, ++tick);
    const needed = chunksFor(first, last).filter((c) => !loadedChunks.has(c));
    // A scroll replaces the queue outright: chunks the viewport has
    // left are never started. (Ones already in flight finish and are
    // cached; cancelling a request mid-flight would save nothing.)
    queue = needed.filter((c) => !inFlight.has(c));
    pump();
  }

  async function loadList(keep: boolean) {
    const token = ++listToken;
    loading.value = true;
    error.value = null;
    try {
      const result = await options.fetchIds();
      if (token !== listToken) return;
      generation += 1;
      ids.value = result.ids;
      total.value = result.total;
      indexById = new Map(result.ids.map((id, i) => [id, i]));
      loadedChunks = new Set();
      lastUsed.clear();
      inFlight.clear();
      queue = [];
      if (!keep) cache.clear();
      else for (const id of [...cache.keys()]) if (!indexById.has(id)) cache.delete(id);
      bump();
      setRange(wanted.first, wanted.last);
    } catch (e) {
      if (token !== listToken) return;
      error.value = e instanceof Error ? e.message : String(e);
      generation += 1;
      ids.value = [];
      total.value = 0;
      indexById = new Map();
      loadedChunks = new Set();
      lastUsed.clear();
      queue = [];
      cache.clear();
      bump();
    } finally {
      if (token === listToken) loading.value = false;
    }
  }

  return {
    ids,
    total,
    loading,
    error,
    resetCount,
    version,
    cache,
    async reset() {
      resetCount.value += 1;
      wanted = { first: 0, last: wanted.last - wanted.first };
      // Rows from the old query must not show under the new one.
      generation += 1;
      queue = [];
      cache.clear();
      await loadList(false);
    },
    refresh: () => loadList(true),
    setRange,
    idAt: (i) => ids.value[i],
    indexOf: (id) => indexById.get(id) ?? -1,
    bookAt: (i) => {
      const id = ids.value[i];
      return id === undefined ? undefined : cache.get(id);
    },
    getById: (id) => cache.get(id),
    idsBetween(a, b) {
      const lo = Math.max(0, Math.min(a, b));
      const hi = Math.min(ids.value.length - 1, Math.max(a, b));
      return hi < lo ? [] : ids.value.slice(lo, hi + 1);
    },
    async ensureLoaded(want) {
      // What is asked for is collected here, not read back from the cache afterwards:
      // the cache is capped, and loading 60,000 rows into it would push the first ones out.
      const got = new Map<number, BookSummary>();
      const missing: number[] = [];
      for (const id of want) {
        const have = cache.get(id);
        if (have) got.set(id, have);
        else missing.push(id);
      }
      for (let i = 0; i < missing.length; i += chunkSize) {
        const books = await options.fetchBooks(missing.slice(i, i + chunkSize));
        for (const b of books) {
          got.set(b.id, b);
          cache.set(b.id, b);
        }
        enforceCap();
      }
      if (missing.length > 0) bump();
      return want.map((id) => got.get(id)).filter((b): b is BookSummary => b !== undefined);
    },
    patch(id, changes) {
      const before = cache.get(id);
      if (!before) return undefined;
      cache.set(id, { ...before, ...changes } as BookSummary);
      bump();
      return before;
    },
    put(book) {
      cache.set(book.id, book);
      bump();
    },
    invalidate(list) {
      const chunks = new Set<number>();
      for (const id of list) {
        const i = indexById.get(id);
        if (i !== undefined) chunks.add(Math.floor(i / chunkSize));
      }
      for (const c of chunks) loadedChunks.delete(c);
      if (chunks.size > 0) setRange(wanted.first, wanted.last);
    },
    pendingCount: () => inFlight.size,
  };
}
