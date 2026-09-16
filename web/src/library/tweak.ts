// Real fetch wrappers for calibre_srv's book-container editor (see
// crates/calibre_srv/src/tweak.rs's own doc) -- EPUB only for this
// first slice. Kept separate from library/api.ts: file read/write
// bodies are raw text, not JSON.

const LIBRARY_ID = "default"; // matches reader/api.ts's own single-library convention

// Percent-encodes each path segment individually and rejoins with a
// literal `/` -- matches reader/api.ts's own `encodeFileName` (axum's
// `{*name}` wildcard route needs each segment decoded, not the whole
// path escaped then un-escaped).
function encodeFileName(name: string): string {
  return name.split("/").map(encodeURIComponent).join("/");
}

export interface TweakSession {
  session_id: string;
  files: string[];
}

export async function openTweakSession(bookId: number): Promise<TweakSession> {
  const url = `/tweak/open/${bookId}/epub/${LIBRARY_ID}`;
  const resp = await fetch(url, { method: "POST" });
  if (!resp.ok) throw new Error(`POST ${url} failed: ${resp.status} ${(await resp.text()) || resp.statusText}`);
  return (await resp.json()) as TweakSession;
}

export async function fetchTweakFile(sessionId: string, name: string): Promise<string> {
  const url = `/tweak/file/${sessionId}/${encodeFileName(name)}`;
  const resp = await fetch(url);
  if (!resp.ok) throw new Error((await resp.text()) || `${resp.status} ${resp.statusText}`);
  return resp.text();
}

export async function saveTweakFile(sessionId: string, name: string, content: string): Promise<void> {
  const url = `/tweak/file/${sessionId}/${encodeFileName(name)}`;
  const resp = await fetch(url, { method: "POST", body: content });
  if (!resp.ok) throw new Error((await resp.text()) || `${resp.status} ${resp.statusText}`);
}

export async function commitTweakSession(sessionId: string): Promise<void> {
  const url = `/tweak/commit/${sessionId}`;
  const resp = await fetch(url, { method: "POST" });
  if (!resp.ok) throw new Error(await resp.text() || `${resp.status} ${resp.statusText}`);
}

export async function discardTweakSession(sessionId: string): Promise<void> {
  await fetch(`/tweak/discard/${sessionId}`, { method: "POST" });
}
