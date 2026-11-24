// A tab left open across a server upgrade still names the old, hashed script files
// of the lazy-loaded routes (the reader, Settings). The server answers those names
// with the app's index page, so the dynamic import fails -- and the route never
// opens until the page is reloaded. The router's error hook reloads on the target
// URL when it sees that failure -- once: a chunk that is gone for good (partial
// deploy, blocked script, offline) would otherwise reload forever.

const STALE_CHUNK = /dynamically imported module|Importing a module script failed|error loading dynamically imported module|Failed to fetch dynamically imported module/i;

/** True when `error` is a lazy route's script failing to load (an old build's file name). */
export function isStaleChunkError(error: unknown): boolean {
  const text = error instanceof Error ? `${error.name} ${error.message}` : String(error);
  return STALE_CHUNK.test(text);
}

const RELOAD_KEY = "staleChunkReload";
/** One reload per target inside this window: a second failure means the file is really gone. */
export const RELOAD_WINDOW_MS = 30_000;

/** The slice of `Storage` the guard needs (tests pass a plain map). */
export interface ReloadMemory {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function sessionMemory(): ReloadMemory | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

/** True when this path was already reloaded for a failed chunk in the last 30 seconds. */
function reloadedRecently(memory: ReloadMemory | null, path: string, now: number): boolean {
  let raw: string | null = null;
  try {
    raw = memory?.getItem(RELOAD_KEY) ?? null;
  } catch {
    return false;
  }
  if (!raw) return false;
  const at = raw.lastIndexOf("|");
  if (at < 0 || raw.slice(0, at) !== path) return false;
  const when = Number(raw.slice(at + 1));
  return Number.isFinite(when) && now >= when && now - when < RELOAD_WINDOW_MS;
}

/** Forget the reload record: call after a navigation succeeded, so a later upgrade can reload again. */
export function clearStaleChunkReload(memory: ReloadMemory | null = sessionMemory()): void {
  try {
    memory?.removeItem(RELOAD_KEY);
  } catch {
    // storage blocked: the guard then falls back to "reload", which is the old behaviour
  }
}

export const STALE_CHUNK_MESSAGE = "This page could not load. Reload the app.";

/**
 * The router's `onError` hook: load the page the user was going to, from the server, when its
 * script is gone -- at most once per target per 30 seconds. A second failure means the file is
 * truly missing, so `giveUp` shows an error instead of reloading forever. Returns true when the
 * error was a stale chunk (reloaded or given up).
 */
export function reloadOnStaleChunk(
  error: unknown,
  target: { fullPath: string },
  assign: (url: string) => void = (url) => window.location.assign(url),
  giveUp: (message: string) => void = showLoadError,
  memory: ReloadMemory | null = sessionMemory(),
  now: number = Date.now(),
): boolean {
  if (!isStaleChunkError(error)) return false;
  if (reloadedRecently(memory, target.fullPath, now)) {
    giveUp(STALE_CHUNK_MESSAGE);
    return true;
  }
  try {
    memory?.setItem(RELOAD_KEY, `${target.fullPath}|${now}`);
  } catch {
    // blocked storage cannot remember the reload; do it once anyway
  }
  assign(target.fullPath);
  return true;
}

/** Replace the app with a plain error (the router has nothing to show for the failed route). */
function showLoadError(message: string): void {
  const root = document.getElementById("app");
  if (!root) return;
  root.textContent = "";
  const box = document.createElement("div");
  box.setAttribute("role", "alert");
  box.style.cssText = "padding:24px;font:14px system-ui,sans-serif";
  box.textContent = message;
  const button = document.createElement("button");
  button.textContent = "Reload";
  button.style.marginLeft = "12px";
  button.onclick = () => {
    clearStaleChunkReload();
    window.location.reload();
  };
  box.append(button);
  root.append(box);
}
