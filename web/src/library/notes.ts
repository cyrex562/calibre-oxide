// Real fetch wrappers for calibre_srv's notes feature (see
// crates/calibre_srv/src/notes.rs's own doc) -- free-form HTML notes
// attachable to a category item (author/tag/series/publisher/
// language), with embedded image resources. Kept separate from
// library/api.ts (which is JSON-only): `POST /set-note` returns a raw
// HTML body, not JSON.

const LIBRARY_ID = "default"; // matches reader/api.ts's own single-library convention

export interface NoteByName {
  item_id: number;
  html: string;
}

export async function fetchNoteByName(field: string, itemName: string): Promise<NoteByName> {
  const url = `/get-note-from-item-val/${encodeURIComponent(field)}/${encodeURIComponent(itemName)}/${LIBRARY_ID}`;
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`GET ${url} failed: ${resp.status} ${resp.statusText}`);
  return (await resp.json()) as NoteByName;
}

// One entry of `POST /set-note`'s `images` map -- `data` is either a
// fresh `data:` URL (a new upload) or an existing
// `/get-note-resource/{scheme}/{digest}` URL (an already-attached
// image being kept, see notes.rs::extract_existing_resource_ref).
// `filename` is required by the server for the former, ignored for
// the latter.
export interface NoteImageSpec {
  data: string;
  filename?: string;
}

/// Returns the saved note's real HTML (server-rewritten resource
/// URLs), matching what a subsequent `fetchNoteByName` would return.
export async function saveNote(field: string, itemId: number, html: string, images: Record<string, NoteImageSpec>): Promise<string> {
  const url = `/set-note/${encodeURIComponent(field)}/${itemId}/${LIBRARY_ID}`;
  const resp = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ html, images }),
  });
  if (!resp.ok) throw new Error(`POST ${url} failed: ${resp.status} ${resp.statusText}`);
  return resp.text();
}
