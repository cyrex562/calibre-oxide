// A deliberately simple whole-spine-file-granularity position scheme
// for this MVP slice, NOT a real EPUB CFI (calibre's own `cfi.pyj` is
// ~1000 lines of range/point CFI computation against live DOM
// measurements -- out of scope here, see issue #499's own doc for
// what's deferred). Stored in the same `cfi` string field
// `book-set/get-last-read-position` already expose (calibre_srv
// itself never parses this field as a real CFI, it's opaque text to
// the backend either way), so this is forward-compatible with a real
// CFI implementation landing later without a schema change.

const PREFIX = "calibre-oxide-simple-pos:";

export interface SimplePosition {
  spineIndex: number;
  frag: string;
}

export function encodePosition(pos: SimplePosition): string {
  return `${PREFIX}${pos.spineIndex}:${encodeURIComponent(pos.frag)}`;
}

export function decodePosition(cfi: string | null | undefined): SimplePosition | null {
  if (!cfi || !cfi.startsWith(PREFIX)) return null;
  const rest = cfi.slice(PREFIX.length);
  const sep = rest.indexOf(":");
  if (sep === -1) return null;
  const spineIndex = Number.parseInt(rest.slice(0, sep), 10);
  if (Number.isNaN(spineIndex)) return null;
  return { spineIndex, frag: decodeURIComponent(rest.slice(sep + 1)) };
}
