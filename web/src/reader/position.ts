// A deliberately simple whole-spine-file-granularity position scheme
// for this MVP slice, NOT a real EPUB CFI (calibre's own `cfi.pyj` is
// ~1000 lines of range/point CFI computation against live DOM
// measurements -- out of scope here, see issue #499's own doc for
// what's deferred). Stored in the same `cfi` string field
// `book-set/get-last-read-position` already expose (lexmechanic_srv
// itself never parses this field as a real CFI, it's opaque text to
// the backend either way), so this is forward-compatible with a real
// CFI implementation landing later without a schema change.

import { getItemCompat } from "../library/legacyStorage";

const PREFIX = "lexmechanic-simple-pos:";
// Positions saved before the rename carry this prefix. They are still read
// (they are stored in the library, so they outlive any one build), never written.
const LEGACY_PREFIX = "calibre-oxide-simple-pos:";

export interface SimplePosition {
  spineIndex: number;
  frag: string;
}

export function encodePosition(pos: SimplePosition): string {
  return `${PREFIX}${pos.spineIndex}:${encodeURIComponent(pos.frag)}`;
}

export function decodePosition(cfi: string | null | undefined): SimplePosition | null {
  if (!cfi) return null;
  const prefix = cfi.startsWith(PREFIX) ? PREFIX : cfi.startsWith(LEGACY_PREFIX) ? LEGACY_PREFIX : null;
  if (prefix === null) return null;
  const rest = cfi.slice(prefix.length);
  const sep = rest.indexOf(":");
  if (sep === -1) return null;
  const spineIndex = Number.parseInt(rest.slice(0, sep), 10);
  if (Number.isNaN(spineIndex)) return null;
  return { spineIndex, frag: decodeURIComponent(rest.slice(sep + 1)) };
}

/**
 * A stable per-browser id, so a reading position can be attributed to
 * this device rather than overwriting another one's.
 *
 * Lives here rather than in a component because both readers -- EPUB
 * and PDF -- need it, and two copies would drift into two different
 * ids for the same browser.
 */
export function deviceId(): string {
  const key = "lexmechanic-device-id";
  let id = getItemCompat(localStorage, key);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(key, id);
  }
  return id;
}
