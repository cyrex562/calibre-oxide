import { afterEach, describe, expect, it, vi } from "vitest";
import {
  checkState,
  createRetitleController,
  describeConfirm,
  describeFailure,
  describeReport,
  describeRows,
  describeSkip,
  describeUndo,
  itemsToApply,
  pageCount,
  pageSlice,
  RETITLE_PAGE_SIZE,
  rowFromServer,
  rowProblem,
  scopeFor,
  visibleRows,
} from "./retitle";
import type { RetitleApi } from "./retitle";
import { orderRows } from "./retitle";
import { fetchRetitleRuns, runRetitle, suggestRetitles, undoRetitle } from "./api";
import type { RetitleReport, RetitleRow, RetitleSuggestions } from "./api";

const row = (over: Partial<RetitleRow> = {}): RetitleRow => ({
  book_id: 1,
  current_title: "Untitled",
  suggested_title: "Dragon Lore",
  reason: "junk",
  confident: true,
  current_file: "Dragon_Lore",
  formats: ["PDF"],
  proposed_file: "Dragon Lore",
  ...over,
});
const suggestions = (rows: RetitleRow[], over: Partial<RetitleSuggestions> = {}): RetitleSuggestions => ({ rename_template: "{title}", total: rows.length, truncated: false, rows, ...over });
const report = (over: Partial<RetitleReport> = {}): RetitleReport => ({ run_id: "r1", books_planned: 2, retitled: 2, embedded: 2, renamed: 2, skipped: [], failed: [], cancelled: false, ...over });

function fakeApi(over: Partial<RetitleApi> = {}): RetitleApi {
  return {
    suggestRetitles: vi.fn(async () => suggestions([row(), row({ book_id: 2, current_title: "Scan", suggested_title: "", reason: "no_suggestion", proposed_file: null }), row({ book_id: 3, current_title: "Fine", suggested_title: "Fine", reason: "ok" })])),
    runRetitle: vi.fn(async () => report()),
    fetchRetitleRuns: vi.fn(async () => ({ runs: [{ run_id: "r0", created_at: 1, books: 5, undone: false }, { run_id: "rx", created_at: 2, books: 1, undone: true }] })),
    undoRetitle: vi.fn(async () => ({ run_id: "r1", titles_restored: 2, files_restored: 2, re_embedded: 2, skipped: [], failed: [], cancelled: false })),
    ...over,
  };
}

describe("rows", () => {
  it("are pre-checked only when the title needs fixing and a suggestion exists", () => {
    expect(rowFromServer(row()).checked).toBe(true);
    expect(rowFromServer(row({ reason: "template_leftover" })).checked).toBe(true);
    expect(rowFromServer(row({ reason: "ok", suggested_title: "Untitled" })).checked).toBe(false);
    expect(rowFromServer(row({ suggested_title: "MR1494.ch4", confident: false })).checked).toBe(false);
  });

  it("orders ready rows first, then ones to check, then ones needing a title, then fine ones", () => {
    const rows = [
      row({ book_id: 1, reason: "ok", current_title: "Fine", suggested_title: "Fine" }),
      row({ book_id: 2, reason: "no_suggestion", suggested_title: "" }),
      row({ book_id: 3, suggested_title: "MR1494.ch4", confident: false }),
      row({ book_id: 4 }),
      row({ book_id: 5 }),
    ].map(rowFromServer);
    expect(orderRows(rows).map((r) => r.bookId)).toEqual([4, 5, 3, 2, 1]);
    const none = rowFromServer(row({ reason: "no_suggestion", suggested_title: "" }));
    expect(none.checked).toBe(false);
    expect(none.title).toBe("");
  });

  it("flag a ticked row with no title, and send only usable ones", () => {
    const ok = rowFromServer(row());
    const blank = { ...rowFromServer(row({ book_id: 2 })), title: "  " };
    const unticked = { ...rowFromServer(row({ book_id: 3 })), checked: false };
    expect(rowProblem(ok)).toBeNull();
    expect(rowProblem(blank)).not.toBeNull();
    expect(rowProblem({ ...blank, checked: false })).toBeNull();
    expect(itemsToApply([ok, blank, unticked])).toEqual([{ book_id: 1, expected_title: "Untitled", title: "Dragon Lore" }]);
  });

  it("send the edited title, trimmed, against the title the user saw", () => {
    const r = rowFromServer(row());
    r.title = "  Dragon Lore, 2nd Edition ";
    expect(itemsToApply([r])).toEqual([{ book_id: 1, expected_title: "Untitled", title: "Dragon Lore, 2nd Edition" }]);
  });
});

describe("filtering and paging", () => {
  const rows = Array.from({ length: 450 }, (_, i) => rowFromServer(row({ book_id: i + 1, reason: i % 3 === 0 ? "template_leftover" : "junk" })));

  it("filters by reason", () => {
    expect(visibleRows(rows, "all")).toHaveLength(450);
    expect(visibleRows(rows, "template_leftover")).toHaveLength(150);
    expect(visibleRows(rows, "ok")).toHaveLength(0);
  });

  it("pages in 200s, and clamps a page out of range", () => {
    expect(RETITLE_PAGE_SIZE).toBe(200);
    expect(pageCount(450)).toBe(3);
    expect(pageCount(0)).toBe(1);
    expect(pageSlice(rows, 0)).toHaveLength(200);
    expect(pageSlice(rows, 2)).toHaveLength(50);
    expect(pageSlice(rows, 9)).toHaveLength(50);
    expect(pageSlice(rows, -3)[0].bookId).toBe(1);
  });

  it("reports the header checkbox state", () => {
    const some = [rowFromServer(row()), { ...rowFromServer(row({ book_id: 2 })), checked: false }];
    expect(checkState(some)).toBe("some");
    expect(checkState(some.map((r) => ({ ...r, checked: true })))).toBe("all");
    expect(checkState(some.map((r) => ({ ...r, checked: false })))).toBe("none");
  });
});

describe("describing", () => {
  it("states what will happen", () => {
    expect(describeConfirm(12, { embed: true, renameFiles: true, renameTemplate: "{title}" })).toBe("12 titles, files rewritten, files renamed. You can undo this run afterwards.");
    expect(describeConfirm(1, { embed: false, renameFiles: false, renameTemplate: "" })).toContain("1 title, files left as they are, files not renamed");
  });
  it("counts rows and notes a capped list", () => {
    expect(describeRows(450, 450, 12, 450, false)).toBe("450 books · 12 selected");
    expect(describeRows(150, 450, 12, 450, false)).toBe("150 books of 450 · 12 selected");
    expect(describeRows(5000, 5000, 5000, 9000, true)).toContain("showing the first 5,000 of 9,000");
  });
  it("summarises a run, skips, failures and an undo", () => {
    expect(describeReport(report())).toBe("2 titles set · 2 books rewritten · 2 books renamed");
    expect(describeReport(report({ embedded: 0, renamed: 0, skipped: [{ book_id: 1, reason: "invalid" }], failed: [{ book_id: 2, stage: "embed", reason: "x", title_set: true }], cancelled: true }))).toBe("2 titles set · 1 skipped · 1 with problems · stopped early");
    expect(describeSkip("changed_since_preview")).toContain("changed since the preview");
    expect(describeSkip("something else")).toBe("something else");
    expect(describeFailure({ stage: "embed", reason: "not a PDF", title_set: true })).toBe("writing it into the files failed (the new title was kept): not a PDF");
    expect(describeUndo({ run_id: "r", titles_restored: 1, files_restored: 1, re_embedded: 1, skipped: [{ book_id: 2, reason: "x" }], failed: [], cancelled: false })).toContain("1 left alone");
  });
  it("scopes to the selection or everything", () => {
    expect(scopeFor("selected", [3, 4])).toEqual({ bookIds: [3, 4] });
    expect(scopeFor("all", [3, 4])).toEqual({ all: true });
  });
});

describe("the controller", () => {
  it("finds, pre-ticks the fixable rows, and applies only ticked rows with their edits", async () => {
    const api = fakeApi();
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    expect(api.suggestRetitles).toHaveBeenCalledWith({ all: true }, "{title}");
    expect(c.state.phase).toBe("ready");
    expect(c.state.rows.map((r) => r.checked)).toEqual([true, false, false]);
    expect(c.checkedCount()).toBe(1);

    c.state.rows[0].title = "Dragon Lore (edited)";
    c.state.rows[1].title = "Typed by hand";
    c.state.rows[1].checked = true;
    const r = await c.apply();
    expect(r?.run_id).toBe("r1");
    expect(api.runRetitle).toHaveBeenCalledWith(
      [
        { book_id: 1, expected_title: "Untitled", title: "Dragon Lore (edited)" },
        { book_id: 2, expected_title: "Scan", title: "Typed by hand" },
      ],
      { embed: false, renameFiles: true, renameTemplate: "{title}" },
      expect.anything(),
    );
    expect(c.state.phase).toBe("done");
    expect(c.state.rows).toEqual([]);
    expect(c.state.runs.map((x) => x.run_id)).toEqual(["r0"]);
  });

  it("does not write titles into the files unless asked: that changes their hashes", () => {
    const c = createRetitleController(fakeApi());
    expect(c.state.embed).toBe(false);
  });

  it("passes the options the user chose", async () => {
    const api = fakeApi();
    const c = createRetitleController(api);
    c.open("selected");
    c.state.embed = false;
    c.state.renameTemplate = "{title} - {authors}";
    await c.find([5, 6]);
    expect(api.suggestRetitles).toHaveBeenCalledWith({ bookIds: [5, 6] }, "{title} - {authors}");
    await c.apply();
    expect(api.runRetitle).toHaveBeenCalledWith(expect.anything(), { embed: false, renameFiles: true, renameTemplate: "{title} - {authors}" }, expect.anything());
  });

  it("does not look for a selected scope with nothing selected", async () => {
    const api = fakeApi();
    const c = createRetitleController(api);
    c.open("selected");
    await c.find([]);
    expect(api.suggestRetitles).not.toHaveBeenCalled();
    expect(c.state.error).toBeTruthy();
  });

  it("refuses to apply with nothing ticked, or a ticked row without a title", async () => {
    const api = fakeApi();
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    c.setAllChecked(false);
    expect(c.canApply()).toBe(false);
    expect(await c.apply()).toBeNull();

    c.state.rows[1].checked = true; // no suggestion, no title yet
    expect(c.canApply()).toBe(false);
    expect(await c.apply()).toBeNull();
    expect(api.runRetitle).not.toHaveBeenCalled();
    c.state.rows[1].title = "Now it has one";
    expect(c.canApply()).toBe(true);
  });

  it("the header checkbox acts on the whole filter, across pages, and never ticks a blank title", async () => {
    const many = Array.from({ length: 450 }, (_, i) => row({ book_id: i + 1, reason: i < 300 ? "junk" : "template_leftover" }));
    many.push(row({ book_id: 999, reason: "no_suggestion", suggested_title: "", proposed_file: null }));
    const c = createRetitleController(fakeApi({ suggestRetitles: vi.fn(async () => suggestions(many)) }));
    c.open("all");
    await c.find([]);
    expect(c.pageRows()).toHaveLength(200);
    c.setAllChecked(false);
    c.setFilter("template_leftover");
    c.setAllChecked(true);
    expect(c.checkedCount()).toBe(150);
    c.setFilter("all");
    c.setAllChecked(true);
    expect(c.checkedCount()).toBe(450);
    expect(c.state.rows.find((r) => r.bookId === 999)?.checked).toBe(false);
  });

  it("changing the filter returns to the first page; a page is clamped", async () => {
    const many = Array.from({ length: 450 }, (_, i) => row({ book_id: i + 1 }));
    const c = createRetitleController(fakeApi({ suggestRetitles: vi.fn(async () => suggestions(many)) }));
    c.open("all");
    await c.find([]);
    c.setPage(2);
    expect(c.pageRows()).toHaveLength(50);
    c.setPage(99);
    expect(c.state.page).toBe(2);
    c.setFilter("junk");
    expect(c.state.page).toBe(0);
  });

  it("notes that file names are stale once the template is edited after finding", async () => {
    const c = createRetitleController(fakeApi());
    c.open("all");
    await c.find([]);
    expect(c.proposedIsStale()).toBe(false);
    c.state.renameTemplate = "{title} - {authors}";
    expect(c.proposedIsStale()).toBe(true);
  });

  it("keeps the rows and shows the error when the run fails to start", async () => {
    const api = fakeApi({ runRetitle: vi.fn(async () => { throw new Error("another job is already running on this library"); }) });
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    expect(await c.apply()).toBeNull();
    expect(c.state.phase).toBe("ready");
    expect(c.state.rows).toHaveLength(3);
    expect(c.state.error).toContain("already running");
  });

  it("shows the error when finding fails", async () => {
    const c = createRetitleController(fakeApi({ suggestRetitles: vi.fn(async () => { throw new Error("boom"); }) }));
    c.open("all");
    await c.find([]);
    expect(c.state.phase).toBe("idle");
    expect(c.state.error).toBe("boom");
  });

  it("undoes a run and forgets it", async () => {
    const api = fakeApi();
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    await c.apply();
    expect(await c.undo("r1")).toBe(true);
    expect(c.state.report).toBeNull();
    expect(c.state.undone).toContain("2 titles put back");
    expect(c.state.phase).toBe("idle");
  });

  it("keeps the result when an undo fails", async () => {
    const api = fakeApi({ undoRetitle: vi.fn(async () => { throw new Error("run r1 was already undone"); }) });
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    await c.apply();
    expect(await c.undo("r1")).toBe(false);
    expect(c.state.report).not.toBeNull();
    expect(c.state.phase).toBe("done");
    expect(c.state.error).toContain("already undone");
  });
});

describe("the client functions", () => {
  afterEach(() => vi.unstubAllGlobals());

  function stubFetch(responses: unknown[]) {
    const calls: { url: string; body: unknown }[] = [];
    const queue = [...responses];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        calls.push({ url, body: init?.body ? JSON.parse(init.body as string) : undefined });
        return new Response(JSON.stringify(queue.shift()), { status: 200, headers: { "Content-Type": "application/json" } });
      }),
    );
    return calls;
  }

  it("sends a dry run for every book, or for the listed ones, with the template when given", async () => {
    const calls = stubFetch([suggestions([]), suggestions([])]);
    await suggestRetitles({ all: true }, "  ");
    await suggestRetitles({ bookIds: [1, 2] }, " {title} ");
    expect(calls[0].url).toContain("/retitle-library/default");
    expect(calls[0].body).toEqual({ dry_run: true, scope: { all: true } });
    expect(calls[1].body).toEqual({ dry_run: true, scope: { book_ids: [1, 2] }, rename_template: "{title}" });
  });

  it("applies the items with the options and follows the job to its report", async () => {
    const calls = stubFetch([{ job_id: 7 }, { id: 7, state: "finished", progress: {}, report: report(), error: null }]);
    const items = [{ book_id: 1, expected_title: "Untitled", title: "Dragon Lore" }];
    const out = await runRetitle(items, { embed: true, renameFiles: false, renameTemplate: "{title}" });
    expect(calls[0].body).toEqual({ dry_run: false, items, embed: true, rename_files: false, rename_template: "{title}" });
    expect(calls[1].url).toContain("/library-jobs/7");
    expect(out.run_id).toBe("r1");
  });

  it("undoes by run id as a job, and lists runs", async () => {
    const calls = stubFetch([{ job_id: 8 }, { id: 8, state: "finished", progress: {}, report: { run_id: "r1", titles_restored: 1, files_restored: 1, re_embedded: 0, skipped: [], failed: [], cancelled: false }, error: null }, { runs: [] }]);
    const out = await undoRetitle("r1");
    expect(calls[0].url).toContain("/unretitle-library/default");
    expect(calls[0].body).toEqual({ run_id: "r1" });
    expect(out.titles_restored).toBe(1);
    await fetchRetitleRuns();
    expect(calls[2].url).toContain("/retitle-runs/default");
  });
});

describe("the current search as a scope", () => {
  it("asks for exactly the books of the search", async () => {
    const api = { suggestRetitles: vi.fn(async () => ({ rename_template: "{title}", total: 0, truncated: false, rows: [] })), runRetitle: vi.fn(), fetchRetitleRuns: vi.fn(async () => ({ runs: [] })), undoRetitle: vi.fn() } as unknown as RetitleApi;
    const c = createRetitleController(api);
    c.open("search");
    await c.find([3, 4, 5]);
    expect(api.suggestRetitles).toHaveBeenCalledWith({ bookIds: [3, 4, 5] }, "{title}");
  });

  it("refuses an empty search instead of fixing every book", async () => {
    const api = { suggestRetitles: vi.fn() } as unknown as RetitleApi;
    const c = createRetitleController(api);
    c.open("search");
    await c.find([]);
    expect(api.suggestRetitles).not.toHaveBeenCalled();
    expect(c.state.error).toBe("No books match the current search.");
  });

  it("drops the rows found for another scope when the scope changes", async () => {
    const api = { suggestRetitles: vi.fn(async () => ({ rename_template: "{title}", total: 1, truncated: false, rows: [{ book_id: 1, current_title: "a", suggested_title: "A", reason: "junk", confident: true, current_file: "a", proposed_file: "A", formats: ["PDF"] }] })) } as unknown as RetitleApi;
    const c = createRetitleController(api);
    c.open("all");
    await c.find([]);
    expect(c.state.rows.length).toBe(1);
    c.setScope("search");
    expect(c.state.rows).toEqual([]);
    expect(c.state.phase).toBe("idle");
    expect(c.state.scope).toBe("search");
  });
});
