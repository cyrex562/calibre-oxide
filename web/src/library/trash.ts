// The library's trash (`.caltrash`): what the Trash dialog shows and asks.
//
// calibre moves a deleted book into a trash folder inside the library
// instead of the OS recycle bin, which does not exist on a network share
// (see crates/lexmechanic_db/src/trash.rs). Entries stay for a number of days
// (14 unless the library says otherwise) and can be restored until then.
//
// All the wording and arithmetic lives here, away from the dialog, so it can
// be tested: what an entry is called, how long it has left, and what the
// confirmations say.

export type TrashKind = "book" | "format";

export interface TrashEntry {
  kind: TrashKind;
  /** Names the entry for restore and delete. */
  key: string;
  book_id: number;
  title: string;
  authors: string;
  /** Upper-case formats the entry holds. */
  formats: string[];
  /** Bytes of book files. */
  size: number;
  /** Unix seconds. */
  deleted_at: number;
  /** Unix seconds: when expiry removes it. */
  expires_at: number;
  /** The folder it came from. */
  original_path: string;
  /** Why it cannot be restored, when it cannot. */
  problem: string | null;
}

export interface TrashListing {
  count: number;
  total_size: number;
  expire_after_seconds: number;
  /** Entries expiry removed while this listing was being made. */
  expired: string[];
  entries: TrashEntry[];
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** `1.4 MB`, the way a file manager says it. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** When it was deleted, as a date the user can read. Local time. */
export function formatDeleted(entry: TrashEntry): string {
  const d = new Date(entry.deleted_at * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** "3 days left", "less than a day left", "expired". `nowSeconds` is injected for tests. */
export function timeLeft(entry: TrashEntry, nowSeconds: number): string {
  const left = entry.expires_at - nowSeconds;
  if (left <= 0) return "expires at the next cleanup";
  const days = Math.floor(left / 86400);
  if (days >= 1) return `${days} day${days === 1 ? "" : "s"} left`;
  const hours = Math.floor(left / 3600);
  if (hours >= 1) return `${hours} hour${hours === 1 ? "" : "s"} left`;
  return "less than an hour left";
}

/** What kind of thing the row is, in the user's words. */
export function describeKind(entry: TrashEntry): string {
  if (entry.kind === "book") return entry.formats.length > 0 ? `Book (${entry.formats.join(", ")})` : "Book";
  return entry.formats.length === 1 ? `Format ${entry.formats[0]}` : `Formats ${entry.formats.join(", ")}`;
}

/** Newest deletion first, which is what you want after a mistake. */
export function sortEntries(entries: TrashEntry[]): TrashEntry[] {
  return [...entries].sort((a, b) => b.deleted_at - a.deleted_at || a.key.localeCompare(b.key));
}

/** A stable id for a row, since a book and a format entry can share a key. */
export function entryId(entry: TrashEntry): string {
  return `${entry.kind}:${entry.key}`;
}

/** What restoring `entry` asks the server. A format entry restores one format at a time. */
export interface RestoreRequest {
  kind: TrashKind;
  key: string;
  format?: string;
}

export function restoreRequests(entry: TrashEntry): RestoreRequest[] {
  if (entry.kind === "book") return [{ kind: "book", key: entry.key }];
  return entry.formats.map((format) => ({ kind: "format", key: entry.key, format }));
}

export function confirmDeleteForever(entries: TrashEntry[]): string {
  if (entries.length === 1) {
    const e = entries[0];
    return `Permanently delete “${e.title}”${e.kind === "format" ? ` (${e.formats.join(", ")})` : ""} (${formatSize(e.size)})? This cannot be undone.`;
  }
  const bytes = entries.reduce((n, e) => n + e.size, 0);
  return `Permanently delete these ${entries.length} items (${formatSize(bytes)})? This cannot be undone.`;
}

export function confirmEmpty(listing: Pick<TrashListing, "count" | "total_size">): string {
  return `Empty the trash? ${listing.count} item${listing.count === 1 ? "" : "s"} (${formatSize(listing.total_size)}) will be permanently deleted. This cannot be undone.`;
}

/** One line for the dialog's header. */
export function describeTrash(listing: TrashListing): string {
  if (listing.count === 0) return "The trash is empty.";
  const days = Math.round(listing.expire_after_seconds / 86400);
  const base = `${listing.count} item${listing.count === 1 ? "" : "s"}, ${formatSize(listing.total_size)}. Items are deleted for good after ${days} day${days === 1 ? "" : "s"}`;
  // Books a duplicate run removed are kept far longer, so its Undo keeps working: say so, or a row's "179 days left" contradicts the header.
  // reduce, not Math.max(...): spreading a hundred thousand entries throws a RangeError.
  const longest = listing.entries.reduce((most, e) => Math.max(most, e.expires_at - e.deleted_at), 0);
  if (longest > listing.expire_after_seconds * 1.5) {
    const long = Math.round(longest / 86400);
    return `${base} (${long} days for books a duplicate run removed, so that run can still be undone).`;
  }
  return `${base}.`;
}

/** What a restore did, for the status line. */
export function describeRestored(results: { title?: string; format?: string; renamed?: [string, string][] }[]): string {
  const renamed = results.flatMap((r) => r.renamed ?? []);
  const what = results.length === 1 ? (results[0].title ? `“${results[0].title}”` : results[0].format ? `the ${results[0].format}` : "1 item") : `${results.length} items`;
  const base = `Restored ${what}.`;
  if (renamed.length === 0) return base;
  return `${base} ${renamed.length === 1 ? "A file" : `${renamed.length} files`} had its name taken since and was restored as ${renamed.map(([, to]) => `“${to}”`).join(", ")}.`;
}
