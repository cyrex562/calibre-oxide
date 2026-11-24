// "This folder has N untracked book files — scan now?"
//
// Opening a folder as a library does not scan it, and on a network share a
// scan takes long, so the app asks. The count comes from
// GET /library-untracked (a walk, no reads); this decides whether to ask and
// what to say. Never starts a scan by itself.

export interface UntrackedCount {
  untracked: number;
  /** The count stopped at its cap: there are at least this many. */
  at_least: boolean;
  /** Book files the library already tracks, among those seen. */
  tracked: number;
  /** Every folder was readable. */
  conclusive: boolean;
}

const n = (v: number) => v.toLocaleString("en-US");

/** The question, or null when there is nothing to ask. */
export function scanPromptText(c: UntrackedCount): string | null {
  if (c.untracked <= 0) return null;
  const count = `${c.at_least ? "at least " : ""}${n(c.untracked)}`;
  return `This folder has ${count} untracked book file${c.untracked === 1 && !c.at_least ? "" : "s"} — scan now?`;
}

/** What the prompt says about cost, so "scan now" is an informed choice. */
export function scanPromptHint(c: UntrackedCount): string {
  const parts = ["Scanning adds them to the library without moving or changing any file."];
  if (c.untracked >= 1000 || c.at_least) parts.push("This many files can take a while, especially on a network share; it runs in the background and can be stopped and resumed.");
  if (!c.conclusive) parts.push("Some folders could not be read, so there may be more.");
  return parts.join(" ");
}

/** The sessionStorage key under which "not now" is remembered for one library. */
export function dismissKey(libraryPath: string): string {
  return `lexmechanic:scan-prompt-dismissed:${libraryPath}`;
}

/**
 * Whether to show the prompt: there is something to scan and the user has not
 * already said "not now" for this library in this session. (A new session asks
 * again: the files are still untracked.)
 */
export function shouldPrompt(c: UntrackedCount | null, dismissed: boolean): boolean {
  return c !== null && c.untracked > 0 && !dismissed;
}

/** A line for the result of the scan the prompt started. */
export function describeScanOutcome(r: { added: number[]; cancelled?: boolean; conclusive: boolean; failed: unknown[] }): string {
  const added = `${n(r.added.length)} book${r.added.length === 1 ? "" : "s"} added`;
  const parts = [added];
  if (r.failed.length) parts.push(`${n(r.failed.length)} could not be read`);
  if (r.cancelled) parts.push("stopped early — scan again to carry on");
  else if (!r.conclusive) parts.push("some folders could not be read");
  return parts.join(" · ");
}
