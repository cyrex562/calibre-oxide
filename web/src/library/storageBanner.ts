/**
 * What the library's storage means for the person using it (#258).
 *
 * `/ajax/library-info` reports the storage tier, the SQLite journal mode,
 * and any signals that another program has the library open. This turns
 * that into at most one banner. It is advice, never a blocker: the
 * library stays usable, and the banner links to nothing.
 */

export interface StorageInUseSignal {
  signal: string;
  blocking: boolean;
  message: string;
}

export interface StorageInfo {
  /** `local_internal`, `local_external` or `network`. */
  tier: string;
  /** `wal` or `delete`. */
  journalMode: string;
  inUse: StorageInUseSignal[];
  /** Set when edits are refused because another program has the library open. */
  writeBlockedReason: string | null;
}

export interface StorageBanner {
  /** `warning` for a refused-writes state, `info` for advice. */
  level: "info" | "warning";
  title: string;
  detail: string;
}

/** Parses the `storage` object of `/ajax/library-info`; null when absent or malformed. */
export function parseStorageInfo(raw: unknown): StorageInfo | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.tier !== "string") return null;
  const inUse = Array.isArray(r.in_use)
    ? (r.in_use as Record<string, unknown>[])
        .filter((s) => s && typeof s.message === "string")
        .map((s) => ({ signal: String(s.signal ?? ""), blocking: s.blocking === true, message: String(s.message) }))
    : [];
  return {
    tier: r.tier,
    journalMode: typeof r.journal_mode === "string" ? r.journal_mode : "",
    inUse,
    writeBlockedReason: typeof r.write_blocked_reason === "string" ? r.write_blocked_reason : null,
  };
}

/** The banner to show, or null when there is nothing to say. */
export function storageBanner(storage: StorageInfo | null): StorageBanner | null {
  if (!storage) return null;
  if (storage.writeBlockedReason) {
    return {
      level: "warning",
      title: "Editing is turned off for this library",
      detail: storage.writeBlockedReason,
    };
  }
  const softSignals = storage.inUse.filter((s) => !s.blocking).map((s) => s.message);
  if (storage.tier === "network") {
    const extra = softSignals.length ? ` ${softSignals.join(" ")}` : "";
    return {
      level: "info",
      title: "This library is on network storage",
      detail:
        "Reads and small edits are fine, but large edits (importing or converting many books) are slower and riskier over a network share. For those, work on a local copy and copy it back afterwards." +
        extra,
    };
  }
  if (softSignals.length) {
    return { level: "warning", title: "Another program may be using this library", detail: softSignals.join(" ") };
  }
  return null;
}
