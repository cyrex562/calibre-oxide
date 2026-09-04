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
