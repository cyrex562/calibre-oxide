// Real JSON shapes produced by calibre_ebooks::render_book /
// calibre_ebooks::reader_json -- see those modules' own doc comments
// for exactly what each field means. Kept as plain interfaces (not
// runtime-validated) since this is a trusted, same-origin server.

export interface BookHash {
  hash: string;
  size: number;
  mtime: number;
}

export interface TocNode {
  // The manifest's own synthetic root TOC node has a null title (its
  // real children carry the actual chapter titles) -- confirmed
  // against a live server response, not assumed.
  title: string | null;
  dest: string | null;
  frag: string | null;
  children: TocNode[];
  dest_exists?: boolean;
  dest_error?: string;
  id: number;
}

export interface Landmark {
  dest: string;
  frag: string;
  title: string;
  type: string;
}

export interface FileInfo {
  is_html: boolean;
  has_maths: boolean;
  is_virtualized: boolean;
  length?: number;
}

export interface BookManifest {
  version: number;
  toc: TocNode;
  book_format: string;
  spine: string[];
  link_uid: string;
  book_hash: BookHash;
  raster_cover_name: string | null;
  title_page_name: string | null;
  has_maths: boolean;
  total_length: number;
  landmarks: Landmark[];
  page_progression_direction: string;
  files: Record<string, FileInfo>;
  metadata?: Record<string, unknown>;
  last_read_positions?: LastReadPosition[];
  annotations_map?: Record<string, unknown[]>;
  // Cache-miss / job-status shaped response instead of a real manifest.
  job_status?: "waiting" | "running" | "finished" | "failed" | "unknown";
  aborted?: boolean;
  traceback?: string;
  job_id?: number;
}

export interface LastReadPosition {
  device: string;
  cfi: string | null;
  pos_frac: number;
  // Confirmed against a live server response, not assumed --
  // `Cache::get_last_read_positions`'s own real field name.
  epoch?: number;
}

// Port of reader_json.rs's serialize_document/serialize_node shape.
export interface SerializedDocument {
  version: number;
  tree: SerializedNode;
  ns_map: string[];
}

export type SerializedNode = SerializedElement | SerializedComment;

export interface SerializedElement {
  n: string; // tag name
  x?: string; // text
  l?: string; // tail
  a?: [string, string][]; // attributes
  c?: SerializedNode[]; // children
}

export interface SerializedComment {
  s: "c";
  x?: string;
  l?: string;
}

export function isComment(node: SerializedNode): node is SerializedComment {
  return (node as SerializedComment).s === "c";
}
