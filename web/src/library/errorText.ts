// What a failure says to a person. The fetch wrappers throw messages written for
// a developer ("GET /plugins/catalog failed: 503 Service Unavailable -- plugins
// are not enabled on this server -- start it with --plugin-dir <path>"), and a
// dead server surfaces as the browser's own "Failed to fetch". Both ended up on
// screen word for word. This turns them into one plain sentence, in one place.

/** The server could not be reached at all: it is down, restarting, or the network is. */
export const UNREACHABLE = "Can't reach the Lexmechanic server. Check that it is running, then try again.";

const NETWORK_FAILURES = [/^failed to fetch$/i, /^load failed$/i, /^networkerror/i, /^network request failed/i, /^the network connection was lost/i, /^fetch failed/i];

/** "GET /ajax/search?... failed: 503 Service Unavailable — detail". */
const HTTP_FAILURE = /^(?:[A-Z]+ )?\S+ failed: (\d{3})(?: ([^—]*?))?(?: — (.*))?$/s;

/** A command-line hint ("start it with --plugin-dir <path>") is for whoever runs the server, not the reader. */
function withoutCliHint(text: string): string {
  return text.replace(/\s*(?:--|—|-)?\s*start it with\s+--\S+(?:\s+<[^>]*>)?\.?/gi, "").trim();
}

/** Whether the failure is "nothing answered", as opposed to "the server said no". */
export function isUnreachable(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error ?? "");
  if (error instanceof TypeError && NETWORK_FAILURES.some((re) => re.test(message))) return true;
  return NETWORK_FAILURES.some((re) => re.test(message.trim()));
}

/**
 * One plain sentence for a failure: an `Error`, a message, or anything thrown.
 * A server's own short explanation is kept ("That folder is inside the library
 * already"); the status line, the URL and command-line flags are not.
 */
export function humanError(error: unknown): string {
  const raw = (error instanceof Error ? error.message : String(error ?? "")).trim();
  if (raw === "") return "Something went wrong.";
  if (isUnreachable(error)) return UNREACHABLE;

  const http = HTTP_FAILURE.exec(raw);
  if (http) {
    const status = Number(http[1]);
    const detail = withoutCliHint((http[3] ?? "").trim());
    if (detail) return /[.!?]$/.test(detail) ? detail : `${detail}.`;
    if (status === 401 || status === 403) return "The server refused this: you do not have permission.";
    if (status === 404) return "The server does not have that (it may have been removed or renamed).";
    if (status === 408 || status === 504) return "The server took too long to answer. Try again.";
    if (status === 413) return "That is too large for the server to accept.";
    if (status === 429) return "The server is busy. Wait a moment and try again.";
    if (status === 501 || status === 503) return "That feature is not turned on for this server.";
    if (status >= 500) return `The server hit a problem (error ${status}). Try again; if it keeps happening, check the server's log.`;
    return `The server could not do that (error ${status}).`;
  }

  return withoutCliHint(raw) || raw;
}
