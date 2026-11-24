// Full-text search -- a separate result shape/backend (crates/lexmechanic_srv/src/fts.rs)
// from the metadata search: no pagination/sort/vl, real snippet/highlight text
// per match, and a real "not enabled yet" state to surface instead of an empty
// result list.

import { ref, watch, type Ref } from "vue";

import { ftsSearch, ftsSnippets, setFtsEnabled } from "./api";
import type { FtsSnippet } from "./types";

export function useFtsSearch(activeQuery: Ref<string>) {
  // Declared before any watcher that reads it: a `const` read before its line
  // throws (temporal dead zone) and blanked the whole view once.
  const ftsMode = ref(false);

  interface FtsDisplayResult {
    bookId: number;
    title: string;
    authors: string;
    formats: string[];
    snippets: FtsSnippet[];
  }

  const ftsLoading = ref(false);
  const ftsError = ref<string | null>(null);
  const ftsNotEnabled = ref(false);
  const ftsEnabling = ref(false);
  const ftsIndexing = ref<{ left: number; total: number } | null>(null);
  const ftsResults = ref<FtsDisplayResult[]>([]);

  /** Which full-text search is the latest: a slow earlier one must not overwrite a newer one's results. */
  let ftsToken = 0;

  async function runFtsSearch() {
    const query = activeQuery.value;
    const token = ++ftsToken;
    ftsError.value = null;
    ftsNotEnabled.value = false;
    ftsIndexing.value = null;
    if (!query) {
      ftsResults.value = [];
      ftsLoading.value = false;
      return;
    }
    ftsLoading.value = true;
    try {
      const outcome = await ftsSearch(query);
      if (token !== ftsToken) return;
      if (!outcome.enabled) {
        ftsNotEnabled.value = true;
        ftsResults.value = [];
        return;
      }
      const { metadata, indexing_status, results } = outcome.result;
      ftsIndexing.value = indexing_status;

      const formatsByBook = new Map<number, string[]>();
      for (const r of results) {
        const list = formatsByBook.get(r.book_id) ?? [];
        if (!list.includes(r.format)) list.push(r.format);
        formatsByBook.set(r.book_id, list);
      }
      const bookIds = [...formatsByBook.keys()];
      const snippetsByBook = await ftsSnippets(bookIds, query);
      if (token !== ftsToken) return;

      ftsResults.value = bookIds.map((bookId) => ({
        bookId,
        title: metadata[String(bookId)]?.title ?? `Book ${bookId}`,
        authors: metadata[String(bookId)]?.authors ?? "",
        formats: formatsByBook.get(bookId) ?? [],
        snippets: snippetsByBook[String(bookId)] ?? [],
      }));
    } catch (e) {
      if (token !== ftsToken) return;
      ftsError.value = e instanceof Error ? e.message : String(e);
      ftsResults.value = [];
    } finally {
      if (token === ftsToken) ftsLoading.value = false;
    }
  }

  watch([activeQuery, ftsMode], () => {
    if (ftsMode.value) void runFtsSearch();
  });

  async function enableFts() {
    ftsEnabling.value = true;
    ftsError.value = null;
    try {
      await setFtsEnabled(true);
      await runFtsSearch();
    } catch (e) {
      ftsError.value = e instanceof Error ? e.message : String(e);
    } finally {
      ftsEnabling.value = false;
    }
  }

  return { ftsMode, ftsLoading, ftsError, ftsNotEnabled, ftsEnabling, ftsIndexing, ftsResults, runFtsSearch, enableFts };
}
