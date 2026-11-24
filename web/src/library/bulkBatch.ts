// Sends a bulk edit to the server in chunks.
//
// Bulk edit used to be one `POST /cdb/set-fields/{id}` per book, four at a
// time. For 30,000 books that is 30,000 requests and, on a network share, as
// many journal syncs. The batch endpoint (`POST /cdb/set-fields-batch`) takes
// many books per request and writes them in one transaction, so the client
// sends about [`BATCH_CHUNK`] books per request.
//
// What each book gets is computed per book (adding a tag gives every book its
// own tag list), so a chunk is sent as *groups*: books whose changes are
// identical share one entry. A "set the publisher" edit is one group per
// chunk; an "add a tag" edit is a group per distinct existing tag list.
//
// Progress is reported per chunk and cancel stops after the chunk that is
// running. A server without the batch route (404) gets the old per-book calls.
//
// Pure of Vue and `fetch`: the two senders are passed in.

import { runPool } from "./pool";
import type { BookFieldChanges } from "./types";

/** Books per request. */
export const BATCH_CHUNK = 200;

/** Requests in flight at once when falling back to one request per book. */
export const FALLBACK_CONCURRENCY = 4;

export interface BulkItem {
  id: number;
  changes: BookFieldChanges;
}

export interface BatchGroup {
  book_ids: number[];
  changes: BookFieldChanges;
}

export interface BatchResponse {
  updated: number[];
  failed: { book_id: number; error: string }[];
}

/** Thrown by a sender when the server has no batch route (an older server). */
export class BatchUnsupportedError extends Error {
  constructor() {
    super("this server has no batch set-fields route");
    this.name = "BatchUnsupportedError";
  }
}

/** Books with identical changes, in the order each change set first appears. */
export function groupByChanges(items: readonly BulkItem[]): BatchGroup[] {
  const groups = new Map<string, BatchGroup>();
  for (const item of items) {
    const key = JSON.stringify(item.changes);
    const group = groups.get(key);
    if (group) group.book_ids.push(item.id);
    else groups.set(key, { book_ids: [item.id], changes: item.changes });
  }
  return [...groups.values()];
}

export interface BulkRunDeps {
  /** One request for a chunk's groups. Throws [`BatchUnsupportedError`] when the route is missing. */
  sendBatch: (groups: BatchGroup[]) => Promise<BatchResponse>;
  /** The old per-book request. */
  sendOne: (id: number, changes: BookFieldChanges) => Promise<unknown>;
  /** Checked before each chunk (and each book in fallback); true stops starting new work. */
  cancelled?: () => boolean;
  onProgress?: (done: number, total: number) => void;
  chunkSize?: number;
}

export interface BulkRunResult {
  /** Books written. */
  updated: number;
  /** One message per book that failed. */
  errors: string[];
  /** Books never started because of cancellation. */
  notStarted: number;
  /** Whether the per-book route was used because the server has no batch one. */
  usedFallback: boolean;
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Sends `items` to the server, chunk by chunk. A failure is recorded for the books it hit and the run goes on. */
export async function runBulkEdit(items: readonly BulkItem[], deps: BulkRunDeps): Promise<BulkRunResult> {
  const chunkSize = Math.max(1, deps.chunkSize ?? BATCH_CHUNK);
  const result: BulkRunResult = { updated: 0, errors: [], notStarted: 0, usedFallback: false };
  const total = items.length;
  let done = 0;
  let at = 0;

  while (at < total) {
    if (deps.cancelled?.()) break;
    const chunk = items.slice(at, at + chunkSize);
    try {
      const response = await deps.sendBatch(groupByChanges(chunk));
      result.updated += response.updated.length;
      for (const f of response.failed) result.errors.push(`Book ${f.book_id}: ${f.error}`);
    } catch (e) {
      if (e instanceof BatchUnsupportedError) {
        result.usedFallback = true;
        break;
      }
      // The whole request failed (a dropped connection): every book in it is unconfirmed.
      for (const item of chunk) result.errors.push(`Book ${item.id}: ${message(e)}`);
    }
    at += chunk.length;
    done += chunk.length;
    deps.onProgress?.(done, total);
  }

  if (result.usedFallback) {
    const rest = items.slice(at);
    const pool = await runPool(
      rest,
      async (item) => {
        try {
          await deps.sendOne(item.id, item.changes);
          result.updated += 1;
        } catch (e) {
          result.errors.push(`Book ${item.id}: ${message(e)}`);
        }
      },
      { concurrency: FALLBACK_CONCURRENCY, cancelled: deps.cancelled, onProgress: (d) => deps.onProgress?.(done + d, total) },
    );
    result.notStarted = pool.skipped;
  } else {
    result.notStarted = total - at;
  }
  return result;
}
