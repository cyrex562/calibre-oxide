import { afterEach, describe, expect, it, vi } from "vitest";
import { createOrganizeController, DEFAULT_ORGANIZE_TEMPLATE, defaultDraft, describePlan, describeReport, describeRun, describeUndo, draftProblem, draftToScope, planKey, planListIsPartial } from "./organize";
import type { OrganizeApi } from "./organize";
import { fetchOrganizeRuns, planOrganize, runOrganize, undoOrganize } from "./api";
import type { OrganizePlan, OrganizeReport } from "./api";

const plan = (over: Partial<OrganizePlan> = {}): OrganizePlan => ({
  plan_fingerprint: "fp1",
  template: DEFAULT_ORGANIZE_TEMPLATE,
  books: 2,
  files: 3,
  conflicts: 1,
  already_organized: 4,
  skipped: [],
  moves: [{ book_id: 1, title: "A", format: "EPUB", from: "a.epub", to: "X/A.epub", suffixed: false }],
  ...over,
});
const report = (over: Partial<OrganizeReport> = {}): OrganizeReport => ({ run_id: "r1", books_planned: 2, books_moved: 2, files_moved: 3, failed: [], cancelled: false, ...over });

function fakeApi(over: Partial<OrganizeApi> = {}): OrganizeApi {
  return {
    planOrganize: vi.fn(async () => plan()),
    runOrganize: vi.fn(async () => report()),
    fetchOrganizeRuns: vi.fn(async () => ({ runs: [{ run_id: "r0", created_at: 1, books: 5, undone: false }, { run_id: "rx", created_at: 2, books: 1, undone: true }] })),
    undoOrganize: vi.fn(async () => ({ run_id: "r1", books_restored: 2, files_restored: 3, skipped: [] })),
    ...over,
  };
}

describe("the draft", () => {
  it("defaults to the selection when there is one, else everything", () => {
    expect(defaultDraft(3).scope).toBe("selected");
    expect(defaultDraft(0).scope).toBe("all");
    expect(defaultDraft().template).toBe("{author_sort}/{title}");
  });

  it("maps to one scope only", () => {
    const d = defaultDraft(2);
    expect(draftToScope(d, [4, 5])).toEqual({ bookIds: [4, 5] });
    expect(draftToScope({ ...d, scope: "folder", pathPrefix: " /Fiction/Old/ " }, [4])).toEqual({ pathPrefix: "Fiction/Old" });
    expect(draftToScope({ ...d, scope: "all" }, [4])).toEqual({});
  });

  it("cannot be previewed with nothing selected or no folder", () => {
    expect(draftProblem({ ...defaultDraft(), scope: "selected" }, [])).not.toBeNull();
    expect(draftProblem({ ...defaultDraft(), scope: "folder", pathPrefix: "  " }, [])).not.toBeNull();
    expect(draftProblem(defaultDraft(), [])).toBeNull();
  });

  it("keys on template and scope, not on id order", () => {
    const d = defaultDraft(2);
    expect(planKey(d, [2, 1])).toBe(planKey(d, [1, 2]));
    expect(planKey(d, [1, 2])).not.toBe(planKey({ ...d, template: "{title}" }, [1, 2]));
    expect(planKey(d, [1, 2])).not.toBe(planKey(d, [1, 3]));
  });
});

describe("describing", () => {
  it("summarises a preview and notes a capped list", () => {
    expect(describePlan(plan())).toBe("2 books (3 files) would move · 4 already in place · 1 name taken, so numbered");
    expect(planListIsPartial(plan())).toBe(true);
    expect(planListIsPartial(plan({ files: 1 }))).toBe(false);
  });
  it("summarises a run and its undo", () => {
    expect(describeReport(report())).toBe("2 books of 2 organized · 3 files moved");
    expect(describeReport(report({ cancelled: true, failed: [{ book_id: 1, reason: "x" }] }))).toContain("stopped early");
    expect(describeUndo({ run_id: "r", books_restored: 1, files_restored: 1, skipped: [{ book_id: 2, reason: "moved" }] })).toContain("1 left alone");
  });
});

describe("the controller", () => {
  it("runs only a preview of exactly this choice", async () => {
    const api = fakeApi();
    const c = createOrganizeController(api);
    c.open(0);
    expect(await c.run([])).toBeNull();
    expect(api.runOrganize).not.toHaveBeenCalled();

    await c.preview([]);
    expect(c.state.phase).toBe("planned");
    expect(api.planOrganize).toHaveBeenCalledWith(DEFAULT_ORGANIZE_TEMPLATE, {});

    const r = await c.run([]);
    expect(r?.run_id).toBe("r1");
    expect(api.runOrganize).toHaveBeenCalledWith(DEFAULT_ORGANIZE_TEMPLATE, {}, "fp1", expect.anything());
    expect(c.state.phase).toBe("done");
    expect(c.state.plan).toBeNull();
    expect(c.state.runs.map((x) => x.run_id)).toEqual(["r0"]);
  });

  it("drops the preview on any edit", async () => {
    const api = fakeApi();
    const c = createOrganizeController(api);
    c.open(0);
    await c.preview([]);
    c.state.draft.template = "{title}";
    c.edited();
    expect(c.state.plan).toBeNull();
    expect(await c.run([])).toBeNull();
    expect(api.runOrganize).not.toHaveBeenCalled();
  });

  it("refuses a run when the selection changed after the preview", async () => {
    const api = fakeApi();
    const c = createOrganizeController(api);
    c.open(2);
    await c.preview([1, 2]);
    expect(c.planIsCurrent([1, 2])).toBe(true);
    expect(await c.run([1, 3])).toBeNull();
    expect(api.runOrganize).not.toHaveBeenCalled();
  });

  it("does not preview an invalid scope", async () => {
    const api = fakeApi();
    const c = createOrganizeController(api);
    c.open(0);
    c.state.draft.scope = "selected";
    await c.preview([]);
    expect(api.planOrganize).not.toHaveBeenCalled();
    expect(c.state.error).toBeTruthy();
  });

  it("voids the preview and shows the error when the server answers 409", async () => {
    const api = fakeApi({ runOrganize: vi.fn(async () => { throw new Error("the plan is no longer what was previewed"); }) });
    const c = createOrganizeController(api);
    c.open(0);
    await c.preview([]);
    expect(await c.run([])).toBeNull();
    expect(c.state.phase).toBe("idle");
    expect(c.state.plan).toBeNull();
    expect(c.state.error).toContain("no longer");
  });

  it("undoes a run and forgets it", async () => {
    const api = fakeApi();
    const c = createOrganizeController(api);
    c.open(0);
    await c.preview([]);
    await c.run([]);
    expect(await c.undo("r1")).toBe(true);
    expect(c.state.report).toBeNull();
    expect(c.state.undone).toContain("2 books put back");
    expect(c.state.phase).toBe("idle");
  });

  it("keeps the result when an undo fails", async () => {
    const api = fakeApi({ undoOrganize: vi.fn(async () => { throw new Error("nope"); }) });
    const c = createOrganizeController(api);
    c.open(0);
    await c.preview([]);
    await c.run([]);
    expect(await c.undo("r1")).toBe(false);
    expect(c.state.report).not.toBeNull();
    expect(c.state.phase).toBe("done");
    expect(c.state.error).toBe("nope");
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
        const body = queue.shift();
        return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
      }),
    );
    return calls;
  }

  it("sends a dry run, with the template and one scope", async () => {
    const calls = stubFetch([plan()]);
    await planOrganize("  {title} ", { bookIds: [1, 2] });
    expect(calls[0].url).toContain("/organize-library/default");
    expect(calls[0].body).toEqual({ template: "{title}", book_ids: [1, 2], dry_run: true });
  });

  it("leaves the template out when blank and sends a folder prefix", async () => {
    const calls = stubFetch([plan()]);
    await planOrganize("", { pathPrefix: "Fiction" });
    expect(calls[0].body).toEqual({ path_prefix: "Fiction", dry_run: true });
  });

  it("executes with the fingerprint and follows the job to its report", async () => {
    const calls = stubFetch([{ job_id: 7 }, { id: 7, state: "finished", progress: {}, report: report(), error: null }]);
    const out = await runOrganize("{title}", {}, "fp9");
    expect(calls[0].body).toEqual({ template: "{title}", dry_run: false, plan_fingerprint: "fp9" });
    expect(calls[1].url).toContain("/library-jobs/7");
    expect(out.run_id).toBe("r1");
  });

  it("undoes by run id as a job, and lists runs", async () => {
    const calls = stubFetch([{ job_id: 8 }, { id: 8, state: "finished", progress: {}, report: { run_id: "r1", books_restored: 1, files_restored: 1, skipped: [] }, error: null }, { runs: [] }]);
    const out = await undoOrganize("r1");
    expect(calls[0].url).toContain("/unorganize-library/default");
    expect(calls[0].body).toEqual({ run_id: "r1" });
    expect(out.books_restored).toBe(1);
    expect(await fetchOrganizeRuns()).toEqual({ runs: [] });
    expect(calls[2].url).toContain("/organize-runs/default");
  });
});

describe("describeRun", () => {
  it("tells a drag-move, a folder rename and a template run apart", () => {
    expect(describeRun({ books: 1, template: "move to Fiction" })).toBe("Moved 1 book to Fiction");
    expect(describeRun({ books: 12, template: "rename folder Fiction to Novels" })).toBe("Renamed folder Fiction → Novels (12 books)");
    expect(describeRun({ books: 60, template: "{author_sort}/{title}" })).toBe("Organized 60 books with {author_sort}/{title}");
  });
  it("falls back to the count when the server did not say", () => {
    expect(describeRun({ books: 2 })).toBe("2 books");
    expect(describeRun({ books: 1, template: "" })).toBe("1 book");
  });
});

describe("the current search as a scope", () => {
  it("is the default when a search is active and nothing is selected; a selection still wins", () => {
    expect(defaultDraft(0, true).scope).toBe("search");
    expect(defaultDraft(0, false).scope).toBe("all");
    expect(defaultDraft(2, true).scope).toBe("selected");
  });

  it("sends the search's books as the scope, and refuses an empty search", () => {
    const d = defaultDraft(0, true);
    expect(draftToScope(d, [4, 5])).toEqual({ bookIds: [4, 5] });
    expect(draftProblem(d, [])).toBe("No books match the current search.");
    expect(draftProblem(d, [4])).toBeNull();
    expect(planKey(d, [4, 5])).not.toBe(planKey(d, [4]));
  });

  it("previews with those ids", async () => {
    const api = { planOrganize: vi.fn(async () => plan()), runOrganize: vi.fn(), fetchOrganizeRuns: vi.fn(async () => ({ runs: [] })), undoOrganize: vi.fn() } as unknown as OrganizeApi;
    const c = createOrganizeController(api);
    c.open(0, true);
    await c.preview([4, 5]);
    expect(api.planOrganize).toHaveBeenCalledWith(DEFAULT_ORGANIZE_TEMPLATE, { bookIds: [4, 5] });
    expect(c.planIsCurrent([4, 5])).toBe(true);
    expect(c.planIsCurrent([4])).toBe(false);
  });
});
