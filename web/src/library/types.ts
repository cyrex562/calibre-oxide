// Real JSON shapes produced by calibre_srv::ajax -- see
// crates/calibre_srv/src/ajax.rs's own doc comment and per-handler
// docs for exactly what each field means. Kept as plain interfaces
// (not runtime-validated) since this is a trusted, same-origin server.

export interface BookSummary {
  id: number;
  title: string;
  authors: string[];
  series?: string | null;
  series_index?: number | null;
  rating: number | null; // 0..5, already halved by ajax::book_json
  tags?: string[];
  pubdate?: string | null;
  timestamp?: string | null;
  last_modified?: string | null;
  cover: string; // /get/cover/{id}
  thumbnail: string; // /get/thumb/{id}
  formats: string[];
  main_format: Record<string, string> | null;
  other_formats: Record<string, string>;
  // The rest of Cache::get_data_as_dict's row (comments, identifiers,
  // publisher, languages, custom columns, ...) passes through
  // untyped -- this interface only names what the MVP UI reads.
  [key: string]: unknown;
}

export interface SearchResult {
  total_num: number;
  sort_order: string;
  offset: number;
  num: number;
  sort: string;
  base_url: string;
  query: string;
  vl: string;
  library_id: string;
  book_ids: number[];
}

export interface CategoryEntry {
  url: string;
  name: string;
  is_category: true;
}

export interface CategoryItem {
  name: string;
  average_rating: number;
  count: number;
  url: string; // /ajax/books_in/{category}/{item_id}
  has_children: boolean;
}

export interface CategoryPage {
  category_name: string;
  base_url: string;
  total_num: number;
  offset: number;
  num: number;
  sort: string;
  sort_order: string;
  subcategories: unknown[];
  items: CategoryItem[];
}

export interface BooksInPage {
  total_num: number;
  sort_order: string;
  offset: number;
  num: number;
  sort: string;
  base_url: string;
  book_ids: number[];
}

export interface FieldMetadataResponse {
  field_metadata: Record<string, unknown>;
  // [key, display_label] pairs -- calibre_db::field_metadata::FieldMetadata::ui_sortable_field_keys.
  sortable_fields: [string, string][];
}

// { name: query } -- calibre_db::cache::Cache::virtual_library_map.
export type VirtualLibraries = Record<string, string>;

// Response shape of `POST /cdb/add-book/...` -- see crates/calibre_srv/src/cdb.rs::add_book.
// `book_id` is present on a real add; `duplicates` is present instead
// when a same-title/author match already exists and `add_duplicates`
// wasn't set.
export interface AddBookResult {
  title: string;
  authors: string[];
  languages: string[];
  filename: string;
  id: string;
  book_id?: number;
  duplicates?: { title: string; authors: string[] }[];
}

// Real shapes from crates/calibre_srv/src/convert.rs. `book-data`'s
// `input_formats`/`output_formats` entries are always upper-case
// extensions (e.g. "EPUB"), matching what `/ajax/book`'s own `formats`
// field would need `.toUpperCase()`-ing to compare against.
export interface ConversionBookData {
  book_id: number;
  title: string;
  authors: string[];
  input_formats: string[];
  output_formats: string[];
}

// `{running: true, percent, msg}` while in flight; once finished,
// `running: false` plus `ok`/`was_aborted`/`traceback`/`log` and (only
// when `ok`) `size`/`fmt`. Real upstream has no live percent/msg yet
// (see convert.rs's own doc) -- always 0.0/"" while running.
export interface ConversionStatus {
  running: boolean;
  percent?: number;
  msg?: string;
  ok?: boolean;
  was_aborted?: boolean;
  traceback?: string;
  log?: string;
  size?: number;
  fmt?: string;
}

// Fields `POST /cdb/set-fields/{book_id}`'s `changes` object accepts
// for this MVP's edit form -- see cdb.rs::value_to_field_string for
// the full set the server understands (this is a subset).
export interface BookFieldChanges {
  title?: string;
  authors?: string[];
  series?: string;
  series_index?: number;
  tags?: string[];
  rating?: number; // 0..5 display scale, halved server-side to 0..10 storage
  comments?: string;
  // `set-fields`'s own special-cased keys (cdb.rs::set_fields_handle) --
  // not plain metadata fields, but accepted in the same `changes` object.
  added_formats?: { ext: string; data_url: string }[];
  removed_formats?: string[];
}
