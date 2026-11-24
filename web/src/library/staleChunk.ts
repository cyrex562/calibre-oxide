// A tab left open across a server upgrade still names the old, hashed script files
// of the lazy-loaded routes (the reader, Settings). The server answers those names
// with the app's index page, so the dynamic import fails -- and the route never
// opens until the page is reloaded. The router's error hook reloads on the target
// URL when it sees that failure.

const STALE_CHUNK = /dynamically imported module|Importing a module script failed|error loading dynamically imported module|Failed to fetch dynamically imported module/i;

/** True when `error` is a lazy route's script failing to load (an old build's file name). */
export function isStaleChunkError(error: unknown): boolean {
  const text = error instanceof Error ? `${error.name} ${error.message}` : String(error);
  return STALE_CHUNK.test(text);
}

/** The router's `onError` hook: load the page the user was going to, from the server, when its script is gone. */
export function reloadOnStaleChunk(error: unknown, target: { fullPath: string }, assign: (url: string) => void = (url) => window.location.assign(url)): boolean {
  if (!isStaleChunkError(error)) return false;
  assign(target.fullPath);
  return true;
}
