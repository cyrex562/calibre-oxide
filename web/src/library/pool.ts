// Runs async work over a list with a few requests at a time.
//
// Bulk edit is client-side fan-out over the per-book set-fields call (a
// server batch endpoint does not exist yet; adding one would be the real fix
// for very large selections). Sequential it takes N round trips back to back;
// this keeps up to `concurrency` in flight, reports progress and can be
// stopped between items.

export interface PoolOptions<T> {
  /** Requests in flight at once. */
  concurrency?: number;
  /** Checked before each item starts; true stops starting new ones (the ones running finish). */
  cancelled?: () => boolean;
  /** Called after each item finishes, with how many have. */
  onProgress?: (done: number, total: number, item: T) => void;
}

export interface PoolResult {
  /** Items that finished (successfully or not). */
  done: number;
  /** Items never started because of cancellation. */
  skipped: number;
}

/** Runs `worker` for each item. A throwing worker is the caller's to catch: wrap it if one failure must not stop the rest. */
export async function runPool<T>(items: readonly T[], worker: (item: T, index: number) => Promise<void>, options: PoolOptions<T> = {}): Promise<PoolResult> {
  const concurrency = Math.max(1, options.concurrency ?? 4);
  let next = 0;
  let done = 0;
  async function lane() {
    for (;;) {
      if (options.cancelled?.()) return;
      const i = next++;
      if (i >= items.length) return;
      await worker(items[i], i);
      done += 1;
      options.onProgress?.(done, items.length, items[i]);
    }
  }
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, lane));
  return { done, skipped: items.length - done };
}
