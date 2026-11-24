import { describe, expect, it, vi } from "vitest";
import { createDuplicatesStore, defaultFilters, filtersActive, filtersToQuery } from "./duplicatesStore";
import type { DuplicatesApi } from "./duplicatesStore";
import { duplicatesQueryString } from "./api";
import type { DuplicateGroup, DuplicatePage, DuplicatePageOrNone, DuplicateReason, DuplicatesQuery } from "./api";

function group(id: string, reason: DuplicateReason = "content", confidence = 1): DuplicateGroup {
  return { id, reason, confidence, suggested_dest: 1, books: [{ book_id: 1, title: `A ${id}`, authors: ["x"] }, { book_id: 2, title: `B ${id}`, authors: ["x"] }] };
}

/** A fake server over a list of groups: filters by reason and pages, like the real route. */
function fakeServer(all: DuplicateGroup[]) {
  let groups = [...all];
  const calls: DuplicatesQuery[] = [];
  const dropped: string[][] = [];
  const api: DuplicatesApi = {
    fetchDuplicateGroups: vi.fn(async (q: DuplicatesQuery = {}): Promise<DuplicatePageOrNone> => {
      calls.push(q);
      const withoutReason = groups.filter((g) => (q.minConfidence === undefined || (g.confidence ?? 0) >= q.minConfidence));
      const selected = withoutReason.filter((g) => !q.reasons || q.reasons.length === 0 || q.reasons.includes(g.reason));
      const facets: Partial<Record<DuplicateReason, number>> = {};
      for (const g of withoutReason) facets[g.reason] = (facets[g.reason] ?? 0) + 1;
      const offset = q.offset ?? 0;
      const limit = q.limit ?? 50;
      const page: DuplicatePage = { has_scan: true, stale: false, generation: 1, scanned_at: 1, total: selected.length, all_total: groups.length, offset, limit, groups: selected.slice(offset, offset + limit), facets: { reason: facets } };
      return page;
    }),
    runDuplicateScan: vi.fn(async (_min, handlers) => {
      handlers?.onStart?.(7);
      handlers?.onProgress?.({ id: 7, kind: "dedupe", state: "running", progress: { phase: "matching", total: 0, seen: 0, hashed: 0, added: 0, already_known: 0, ignored: 0, enriched: 0, errors: 0, bytes_hashed: 0, cancelled: false }, report: null, error: null });
      return { generation: 1, group_count: groups.length, facets: { reason: {} } };
    }),
    dropDuplicateGroups: vi.fn(async (ids: string[]) => {
      dropped.push(ids);
      groups = groups.filter((g) => !ids.includes(g.id ?? ""));
      return { dropped: ids.length, remaining: groups.length, stale: false };
    }),
    markNotDuplicates: vi.fn(async (t) => {
      groups = groups.filter((g) => g.id !== t.groupId);
      return { exemption: { id: `ex-${t.groupId}`, created_at: 1, members: [] }, pairs: 1, dropped_group_ids: [t.groupId ?? ""], needs_rescan: false };
    }),
    fetchExemptions: vi.fn(async () => ({ exemptions: [], count: 0 })),
    removeExemption: vi.fn(async () => ({ removed: 1, needs_rescan: true })),
  };
  return { api, calls, dropped, setGroups: (g: DuplicateGroup[]) => (groups = g) };
}

const many = (n: number, reason: DuplicateReason = "content") => Array.from({ length: n }, (_, i) => group(`g${String(i).padStart(3, "0")}`, reason, 1 - i / 1000));

describe("filters", () => {
  it("starts unfiltered and says when a filter narrows the list", () => {
    expect(filtersActive(defaultFilters())).toBe(false);
    expect(filtersActive({ ...defaultFilters(), reasons: ["content"] })).toBe(true);
    expect(filtersActive({ ...defaultFilters(), minConfidence: 0.9 })).toBe(true);
    expect(filtersActive({ ...defaultFilters(), path: " " })).toBe(false);
    expect(filtersActive({ ...defaultFilters(), path: "Fiction/" })).toBe(true);
  });

  it("turns filters into the query the server understands, leaving defaults out of the URL", () => {
    expect(duplicatesQueryString({})).toBe("");
    const q = filtersToQuery({ ...defaultFilters(), reasons: ["content", "pdf_resave"], minConfidence: 0.8, path: " Sci Fi/ ", sort: "size", order: "asc" }, 50, 25);
    expect(duplicatesQueryString(q)).toBe("?offset=50&limit=25&reason=content%2Cpdf_resave&min_confidence=0.8&path=Sci+Fi%2F&sort=size&order=asc");
    expect(duplicatesQueryString(filtersToQuery(defaultFilters(), 0, 25))).toBe("?limit=25&order=desc");
  });
});

describe("the store pages through the server's scan", () => {
  it("loads the first page and the facets", async () => {
    const server = fakeServer([...many(30), group("p1", "pdf_resave", 0.85)]);
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    expect(store.state.hasScan).toBe(true);
    expect(store.state.groups).toHaveLength(25);
    expect(store.state.total).toBe(31);
    expect(store.state.facets).toEqual({ content: 30, pdf_resave: 1 });
    expect(store.pageCount.value).toBe(2);
    expect(store.rangeLabel.value).toBe("1–25 of 31");
  });

  it("moves between pages by offset", async () => {
    const server = fakeServer(many(60));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    await store.goToPage(3);
    expect(server.calls[server.calls.length - 1]?.offset).toBe(50);
    expect(store.state.groups).toHaveLength(10);
    expect(store.pageNumber.value).toBe(3);
    await store.goToPage(99);
    expect(store.pageNumber.value).toBe(3);
    await store.goToPage(0);
    expect(store.pageNumber.value).toBe(1);
  });

  it("a filter change goes back to the first page and asks the server to filter", async () => {
    const server = fakeServer([...many(30), ...many(5, "title_author_exact").map((g) => ({ ...g, id: `t${g.id}` }))]);
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    await store.goToPage(2);
    await store.toggleReason("title_author_exact");
    expect(store.state.offset).toBe(0);
    expect(server.calls[server.calls.length - 1]?.reasons).toEqual(["title_author_exact"]);
    expect(store.state.total).toBe(5);
    // Facets still show every tier, so the chips say what ticking another would add.
    expect(store.state.facets.content).toBe(30);
    await store.toggleReason("title_author_exact");
    expect(store.state.filters.reasons).toEqual([]);
    expect(store.state.total).toBe(35);
    await store.setFilters({ minConfidence: 0.99 });
    expect(server.calls[server.calls.length - 1]?.minConfidence).toBe(0.99);
    await store.resetFilters();
    expect(filtersActive(store.state.filters)).toBe(false);
  });

  it("opens an existing scan without rescanning, and scans when there is none", async () => {
    const server = fakeServer(many(3));
    const store = createDuplicatesStore(server.api, 25);
    await store.open();
    expect(server.api.runDuplicateScan).not.toHaveBeenCalled();
    expect(store.state.groups).toHaveLength(3);

    const none: DuplicatesApi = { ...server.api, fetchDuplicateGroups: vi.fn().mockResolvedValueOnce({ has_scan: false }).mockImplementation(server.api.fetchDuplicateGroups) };
    const fresh = createDuplicatesStore(none, 25);
    await fresh.open();
    expect(none.runDuplicateScan).toHaveBeenCalledTimes(1);
    expect(fresh.state.groups).toHaveLength(3);
    expect(fresh.state.scanning).toBe(false);
  });

  it("shows the scan job's progress while it runs", async () => {
    const server = fakeServer(many(2));
    const seen: string[] = [];
    const api: DuplicatesApi = {
      ...server.api,
      runDuplicateScan: vi.fn(async (_min, handlers) => {
        handlers?.onStart?.(3);
        handlers?.onProgress?.({ id: 3, kind: "dedupe", state: "running", progress: { phase: "describing", total: 10, seen: 4, hashed: 0, added: 0, already_known: 0, ignored: 0, enriched: 0, errors: 0, bytes_hashed: 0, cancelled: false }, report: null, error: null });
        seen.push(store.state.scanProgress);
        return { generation: 1, group_count: 2, facets: { reason: {} } };
      }),
    };
    const store = createDuplicatesStore(api, 25);
    await store.scan();
    expect(seen).toEqual(["Preparing groups… 4 of 10"]);
    expect(store.state.scanProgress).toBe("");
    expect(store.state.groups).toHaveLength(2);
  });
});

describe("acting on a group keeps the user's place", () => {
  it("dismissing removes the group at once, tells the server, and refills the same page", async () => {
    const server = fakeServer(many(60));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    await store.goToPage(2);
    const target = store.state.groups[3];
    const ok = await store.dismiss(target);
    expect(ok).toBe(true);
    expect(store.state.offset).toBe(25);
    expect(store.state.groups).toHaveLength(25);
    expect(store.state.groups.some((g) => g.id === target.id)).toBe(false);
    expect(store.state.total).toBe(59);
    expect(store.state.exemptions.map((e) => e.id)).toEqual([`ex-${target.id}`]);
    // No rescan: just the page re-read.
    expect(server.api.runDuplicateScan).not.toHaveBeenCalled();
  });

  it("a group resolved elsewhere (merge, delete) is dropped on the server too", async () => {
    const server = fakeServer(many(30));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    const [a, b] = [store.state.groups[0], store.state.groups[1]];
    await store.resolved([a.id!, b.id!]);
    expect(server.dropped).toEqual([[a.id, b.id]]);
    expect(store.state.total).toBe(28);
    expect(store.state.groups).toHaveLength(25);
    expect(store.state.groups.map((g) => g.id)).not.toContain(a.id);
  });

  it("emptying the last page steps back to the previous one", async () => {
    const server = fakeServer(many(26));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    await store.goToPage(2);
    expect(store.state.groups).toHaveLength(1);
    await store.resolved([store.state.groups[0].id!]);
    expect(store.pageNumber.value).toBe(1);
    expect(store.state.groups).toHaveLength(25);
  });

  it("local removal adjusts the counts and the tier facets before the server answers", async () => {
    const server = fakeServer([...many(3), group("p", "pdf_resave", 0.85)]);
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    store.removeLocal(["p"]);
    expect(store.state.total).toBe(3);
    expect(store.state.facets.pdf_resave).toBe(0);
  });

  it("undoing a dismissal rescans, because the groups it hid are not in the scan", async () => {
    const server = fakeServer(many(2));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    await store.undoDismissal("ex-1");
    expect(server.api.removeExemption).toHaveBeenCalledWith({ id: "ex-1" });
    expect(server.api.runDuplicateScan).toHaveBeenCalledTimes(1);
  });

  it("an error from the server is shown, not thrown", async () => {
    const server = fakeServer(many(2));
    const store = createDuplicatesStore({ ...server.api, markNotDuplicates: vi.fn().mockRejectedValue(new Error("boom")) }, 25);
    await store.load();
    expect(await store.dismiss(store.state.groups[0])).toBe(false);
    expect(store.state.error).toBe("boom");
    expect(store.state.groups).toHaveLength(2);
  });
});

describe("which copy to keep", () => {
  it("is the server's suggestion until the person picks another", async () => {
    const server = fakeServer(many(2));
    const store = createDuplicatesStore(server.api, 25);
    await store.load();
    const g = store.state.groups[0];
    expect(store.keeperFor(g)).toBe(1);
    store.chooseKeeper(g, 2);
    expect(store.keeperFor(g)).toBe(2);
    expect(store.state.keepers[g.id!]).toBe(2);
    store.chooseKeeper(g, 1);
    expect(store.keeperFor(g)).toBe(1);
    expect(store.state.keepers[g.id!]).toBeUndefined();
  });
});

describe("out-of-order replies", () => {
  it("ignores a page that arrives after a newer request was made", async () => {
    const server = fakeServer(many(120));
    const real = server.api.fetchDuplicateGroups as ReturnType<typeof vi.fn>;
    const impl = real.getMockImplementation() as (q?: DuplicatesQuery) => Promise<DuplicatePageOrNone>;
    const gates: (() => void)[] = [];
    let call = 0;
    real.mockImplementation(async (q: DuplicatesQuery = {}) => {
      const mine = ++call;
      const page = await impl(q);
      // The first request answers last.
      if (mine === 1) await new Promise<void>((r) => gates.push(r));
      return page;
    });
    const store = createDuplicatesStore(server.api, 50);
    const first = store.load(); // offset 0, slow
    store.state.offset = 50;
    await store.load(); // the newer request
    expect(store.state.groups[0].id).toBe("g050");
    gates.forEach((g) => g());
    await first;
    // The late reply for page 0 did not overwrite page 2, and loading is not stuck on.
    expect(store.state.groups[0].id).toBe("g050");
    expect(store.state.loading).toBe(false);
  });
});
