// The change-log "behind" warning.
//
// Every edit reaches metadata.db first and the library's change log second.
// When the second step fails (a full disk, a share that dropped) the edit is
// kept and the server leaves a marker: the log is missing changes that only
// the database has, so rebuilding from it would lose them. This turns the
// server's status (`GET /change-log/status`) into the banner that says so.

export interface ChangeLogStatus {
  behind: boolean;
  /** How many appends failed. */
  missed: number;
  /** Unix seconds of the first failure. */
  firstAt: number | null;
  lastError: string;
}

/** Parses the status route's JSON; null when it is not that (an older server has no route). */
export function parseChangeLogStatus(raw: unknown): ChangeLogStatus | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.behind !== "boolean") return null;
  return {
    behind: r.behind,
    missed: typeof r.missed === "number" ? r.missed : 0,
    firstAt: typeof r.first_at === "number" ? r.first_at : null,
    lastError: typeof r.last_error === "string" ? r.last_error : "",
  };
}

export interface ChangeLogBanner {
  /** The sentence shown, without the Repair button. */
  text: string;
}

/** The warning to show, or null when the log is in step (or nothing is known). */
export function changeLogBanner(status: ChangeLogStatus | null): ChangeLogBanner | null {
  if (!status || !status.behind) return null;
  const changes = status.missed === 1 ? "1 change" : `${status.missed.toLocaleString("en-US")} changes`;
  const why = status.lastError ? ` (last error: ${status.lastError})` : "";
  return { text: `The library's change log missed ${changes}${why}.` };
}
