// Browser storage keys written before the project was renamed.
//
// Every key is now `lexmechanic.<name>` or `lexmechanic-<name>`. A browser
// (or the desktop app's web view) that ran an older build still holds the
// same setting under `calibre-oxide.<name>` / `calibre-oxide-<name>`. Reading
// goes through `getItemCompat`: the new key first, then the old one, and a
// value found under the old key is written to the new one so the next read
// does not need the fallback.

type StorageLike = Pick<Storage, "getItem" | "setItem"> & Partial<Pick<Storage, "removeItem">>;

const NEW_PREFIX = "lexmechanic";
const OLD_PREFIX = "calibre-oxide";

/** The name `key` had before the rename, or null for a key that is not ours. */
export function legacyKey(key: string): string | null {
  return key.startsWith(NEW_PREFIX) && /^[.\-:]/.test(key.slice(NEW_PREFIX.length)) ? OLD_PREFIX + key.slice(NEW_PREFIX.length) : null;
}

/** `storage.getItem(key)`, falling back to the pre-rename key and moving what it finds. */
export function getItemCompat(storage: StorageLike, key: string): string | null {
  const current = storage.getItem(key);
  if (current !== null) return current;
  const old = legacyKey(key);
  if (old === null) return null;
  const value = storage.getItem(old);
  if (value === null) return null;
  try {
    storage.setItem(key, value);
    storage.removeItem?.(old);
  } catch {
    // A full or read-only store: the value is still returned this time.
  }
  return value;
}
