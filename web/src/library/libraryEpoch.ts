// A counter of "something may have changed the library", for the library view
// that stays alive (<keep-alive>) while the reader or settings are open.
//
// The server does not push change events, so the page keeps its own tally:
// every successful write request bumps it (see jsonFetch in ./api.ts), and a
// books-added event from the desktop shell does too. The library view notes
// the value when it is deactivated and, when it comes back, reloads its list
// only if the value moved (or it has been away long enough that another
// window could have changed things).

let epoch = 0;

/** Something wrote to the library. */
export function markLibraryChanged(): void {
  epoch += 1;
}

export function libraryEpoch(): number {
  return epoch;
}

/** Away this long, the list is reloaded on return even if this window changed nothing. */
export const STALE_AFTER_MS = 5 * 60 * 1000;

/** Whether a view deactivated at (`epochThen`, `at`) should reload when shown again at `now`. */
export function needsRefresh(epochThen: number, at: number, now: number = Date.now()): boolean {
  return epoch !== epochThen || now - at >= STALE_AFTER_MS;
}

// Books the desktop shell added by drop, while no library view was listening.
if (typeof window !== "undefined") window.addEventListener("lexmechanic:books-added", markLibraryChanged);
