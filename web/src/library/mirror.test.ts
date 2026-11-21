import { describe, expect, it, vi } from "vitest";
import {
  browseVisible,
  conflictsNote,
  conflictText,
  createMirrorController,
  defaultDraft,
  describePlan,
  describeProgress,
  describeReport,
  describeScope,
  describeTarget,
  draftProblem,
  draftToRequest,
  draftToScope,
  formatBytes,
  listIsPartial,
  looksAbsolute,
  networkNote,
  planKey,
  reachableLabel,
  targetScopeKind,
} from "./mirror";
import type { MirrorApi, MirrorEnv } from "./mirror";
import type { LibraryJobProgress, MirrorPlan, MirrorReport, MirrorTarget } from "./api";

const count = (files = 0, bytes = 0) => ({ files, bytes });
const kindTotals = () => ({ copy_new: count(), update: count(), unchanged: count(), remove: count(), conflict: count() });

const plan = (over: Partial<MirrorPlan> = {}): MirrorPlan => ({
  fingerprint: "fp1",
  mode: "travel",
  target: "D:\\Library",
  new_target: true,
  tier: "local_external",
  network: false,
  books: 3,
  scope: "the whole library",
  totals: { copy_new: count(12, 3 * 1024 * 1024), update: count(2, 1024), unchanged: count(400), remove: count(1, 5), conflict: count(), bytes_to_copy: 3 * 1024 * 1024 + 1024, by_kind: { format: kindTotals() } },
  items: { copy_new: [{ path: "A/a.epub", kind: "format", bytes: 10, reason: null }], update: [], remove: [], conflict: [] },
  truncated: { copy_new: false, update: false, remove: false, conflict: false },
  copy_edits: { count: 0, paths: [] },
  missing_sources: { count: 0, paths: [] },
  ...over,
});

const report = (over: Partial<MirrorReport> = {}): MirrorReport => ({ run_id: "r1", mode: "travel", target: "D:\\Library", copied: 12, updated: 2, unchanged: 400, removed: 0, bytes_copied: 2048, conflicts: [], failures: [], cancelled: false, edits_set_aside: 0, trash: null, complete: true, ...over });

const target = (over: Partial<MirrorTarget> = {}): MirrorTarget => ({
  target: "Z:\\Library",
  mode: "mirror",
  scope: "all",
  remove_missing: true,
  overwrite_changed: false,
  last_run_at: 1_700_000_000,
  last_run_id: "r0",
  last_result: { complete: true, copied: 5, updated: 1, removed: 0, conflicts: 0, failures: 0 },
  reachable: true,
  ...over,
});

function fakeApi(over: Partial<MirrorApi> = {}): MirrorApi {
  return {
    planMirror: vi.fn(async () => plan()),
    runMirror: vi.fn(async () => report()),
    fetchMirrorTargets: vi.fn(async () => [target()]),
    forgetMirrorTarget: vi.fn(async () => []),
    ...over,
  };
}

const desktop: MirrorEnv = { isDesktop: () => true, pickFolder: vi.fn(async () => "D:\\Picked") };
const browser: MirrorEnv = { isDesktop: () => false, pickFolder: vi.fn(async () => null) };

describe("the draft", () => {
  it("opens as a travel copy of the selection when there is one, else everything", () => {
    expect(defaultDraft(3)).toMatchObject({ mode: "travel", scope: "selected", removeMissing: true, overwriteChanged: false });
    expect(defaultDraft(0).scope).toBe("all");
  });

  it("needs a full path to a folder", () => {
    const d = { ...defaultDraft(), target: "" };
    expect(draftProblem(d, [])).toMatch(/Choose the folder/);
    expect(draftProblem({ ...d, target: "Library" }, [])).toMatch(/whole path/);
    expect(draftProblem({ ...d, target: "D:\\Library\\..\\x" }, [])).toMatch(/\.\./);
    for (const ok of ["D:\\Library", "D:/Library", "\\\\nas\\share\\Library", "/mnt/usb/Library", "  Z:\\Library  "]) {
      expect(looksAbsolute(ok), ok).toBe(true);
      expect(draftProblem({ ...d, target: ok }, []), ok).toBeNull();
    }
    expect(looksAbsolute("\\nas\\x")).toBe(false);
    expect(looksAbsolute("C:foo")).toBe(false);
  });

  it("cannot use a selection that is empty or a folder that is blank (travel only)", () => {
    const d = { ...defaultDraft(), target: "D:\\L" };
    expect(draftProblem({ ...d, scope: "selected" }, [])).toMatch(/No books are selected/);
    expect(draftProblem({ ...d, scope: "selected" }, [1])).toBeNull();
    expect(draftProblem({ ...d, scope: "folder", folderPrefix: " / " }, [])).toMatch(/Enter the folder/);
    // A mirror ignores scope: it is always the whole library.
    expect(draftProblem({ ...d, mode: "mirror", scope: "selected" }, [])).toBeNull();
  });
});

describe("scope mapping", () => {
  it("maps the choice to the server's scope shapes", () => {
    const d = defaultDraft(2);
    expect(draftToScope(d, [4, 5])).toEqual({ book_ids: [4, 5] });
    expect(draftToScope({ ...d, scope: "folder", folderPrefix: " /Fiction/Old/ " }, [4])).toEqual({ folder_prefix: "Fiction/Old" });
    expect(draftToScope({ ...d, scope: "all" }, [4])).toEqual({ all: true });
  });

  it("makes a backup mirror the whole library whatever the scope says", () => {
    expect(draftToScope({ ...defaultDraft(2), mode: "mirror", scope: "selected" }, [1, 2])).toEqual({ all: true });
  });

  it("builds the request with the trimmed target and the options", () => {
    const d = { ...defaultDraft(0), target: "  D:\\Library ", removeMissing: false, overwriteChanged: true };
    expect(draftToRequest(d, [])).toEqual({ target: "D:\\Library", mode: "travel", scope: { all: true }, remove_missing: false, overwrite_changed: true });
  });

  it("keys on what matters, not on id order or spacing", () => {
    const d = { ...defaultDraft(2), target: "D:\\L" };
    expect(planKey(d, [2, 1])).toBe(planKey(d, [1, 2]));
    expect(planKey(d, [1, 2])).not.toBe(planKey(d, [1, 3]));
    expect(planKey(d, [1, 2])).toBe(planKey({ ...d, target: "  D:\\L " }, [1, 2]));
    expect(planKey(d, [1, 2])).not.toBe(planKey({ ...d, overwriteChanged: true }, [1, 2]));
    expect(planKey(d, [1, 2])).not.toBe(planKey({ ...d, mode: "mirror" }, [1, 2]));
  });
});

describe("describing a plan", () => {
  it("summarises new, updated, unchanged, to remove, conflicts and bytes", () => {
    expect(describePlan(plan())).toBe("12 new · 2 updated · 400 unchanged · 1 to remove · 3.0 MB to copy");
    const withConflict = plan({ totals: { ...plan().totals, conflict: count(2), copy_new: count(), update: count(), bytes_to_copy: 0 } });
    expect(describePlan(withConflict)).toBe("0 new · 0 updated · 400 unchanged · 1 to remove · 2 conflicts · nothing to copy");
    expect(describePlan(plan({ totals: { ...plan().totals, conflict: count(1) } }))).toContain("1 conflict ·");
  });

  it("formats sizes", () => {
    expect(formatBytes(12)).toBe("12 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3.0 GB");
  });

  it("knows a capped list from a complete one", () => {
    expect(listIsPartial(plan(), "copy_new")).toBe(true); // 12 files in the totals, 1 listed
    expect(listIsPartial(plan({ totals: { ...plan().totals, copy_new: count(1) } }), "copy_new")).toBe(false);
    expect(listIsPartial(plan({ truncated: { copy_new: true, update: false, remove: false, conflict: false } }), "copy_new")).toBe(true);
  });

  it("describes the result, including what was skipped or failed", () => {
    expect(describeReport(report())).toBe("12 files copied · 2 updated · 2 KB written");
    const r = report({ removed: 3, conflicts: [{ path: "metadata.db", message: "x" }], failures: [{ path: "a", message: "b" }], cancelled: true });
    expect(describeReport(r)).toBe("12 files copied · 2 updated · 3 moved to .mirror-trash · 2 KB written · 1 conflict skipped · 1 file could not be copied · stopped early — run it again to continue");
  });

  it("follows the job's phases", () => {
    const p = (phase: string, over: Partial<LibraryJobProgress> = {}): LibraryJobProgress => ({ phase, cancelled: false, total: 10, seen: 0, hashed: 0, added: 3, already_known: 0, ignored: 0, enriched: 0, errors: 1, bytes_hashed: 2048, ...over });
    expect(describeProgress(p("mirror_files"))).toBe("Copying… 4 of 10 files · 2 KB");
    expect(describeProgress(p("mirror_databases"))).toMatch(/Writing the databases/);
    expect(describeProgress(p("mirror_removing"))).toMatch(/Setting aside/);
    expect(describeProgress(p(""))).toBe("Starting…");
  });
});

describe("conflicts", () => {
  it("says plainly what a changed metadata.db means", () => {
    expect(conflictText({ path: "metadata.db", kind: "database", bytes: 1, reason: "x" })).toBe("metadata.db in the copy was changed since the last refresh, probably edited on the other computer. Refreshing would discard those edits.");
  });

  it("uses the server's reason for any other file", () => {
    expect(conflictText({ path: "A/b.epub", kind: "format", bytes: 1, reason: "a different file is already there" })).toBe("A/b.epub: a different file is already there.");
    expect(conflictText({ path: "A/b.epub", kind: "format", bytes: 1, reason: null })).toBe("A/b.epub in the copy was changed since the last refresh.");
  });

  it("explains skipped conflicts once, and how to overwrite", () => {
    expect(conflictsNote(plan())).toBeNull();
    const note = conflictsNote(plan({ totals: { ...plan().totals, conflict: count(2) }, copy_edits: { count: 4, paths: [] } }));
    expect(note).toMatch(/2 files in the copy changed/);
    expect(note).toMatch(/4 changes of its own/);
    expect(note).toMatch(/Overwrite files changed in the copy/);
  });
});

describe("a network folder", () => {
  it("is said to be written file by file, with no database opened there", () => {
    expect(networkNote(plan())).toBeNull();
    expect(networkNote(plan({ network: true, tier: "network" }))).toMatch(/file by file.*never opened over the network/);
  });
});

describe("remembered targets", () => {
  it("shows when each last ran and how it went", () => {
    const line = describeTarget(target());
    expect(line).toMatch(/last run/);
    expect(line).toMatch(/5 new, 1 updated/);
    expect(describeTarget(target({ last_result: { complete: false, error: "the share went away" } }))).toMatch(/failed: the share went away/);
    expect(describeTarget(target({ last_result: { complete: false, cancelled: true, copied: 2, updated: 0, conflicts: 1, failures: 0 } }))).toMatch(/1 conflict, stopped early/);
    expect(describeTarget(target({ last_run_at: 0, last_result: null }))).toBe("never run");
  });

  it("says whether the folder answers", () => {
    expect(reachableLabel(target())).toBe("reachable");
    expect(reachableLabel(target({ reachable: false }))).toBe("not connected");
  });

  it("describes and restores scopes", () => {
    expect(describeScope("all")).toBe("the whole library");
    expect(describeScope({ book_ids: [1, 2] })).toBe("2 selected books");
    expect(describeScope({ folder_prefix: "Fiction" })).toBe("the folder Fiction");
    expect(describeScope({ too_large: 5000 })).toBe("5,000 selected books");
    expect(targetScopeKind(target())).toEqual({ scope: "all", folderPrefix: "", needsSelection: false });
    expect(targetScopeKind(target({ scope: { folder_prefix: "F" } }))).toEqual({ scope: "folder", folderPrefix: "F", needsSelection: false });
    expect(targetScopeKind(target({ scope: { book_ids: [1] } })).scope).toBe("selected");
    expect(targetScopeKind(target({ scope: { too_large: 9 } })).needsSelection).toBe(true);
  });

  it("loads, refreshes (prefilling the draft and previewing) and forgets", async () => {
    const api = fakeApi();
    const c = createMirrorController(api, browser);
    await c.loadTargets();
    expect(c.state.targets).toHaveLength(1);

    await c.refresh(target({ mode: "travel", scope: { book_ids: [7, 8] }, remove_missing: false, overwrite_changed: false, target: "D:\\Laptop" }), [1]);
    expect(c.state.draft).toMatchObject({ mode: "travel", target: "D:\\Laptop", scope: "selected", removeMissing: false });
    // The remembered books, not today's selection, are what the preview and run use.
    expect(api.planMirror).toHaveBeenCalledWith({ target: "D:\\Laptop", mode: "travel", scope: { book_ids: [7, 8] }, remove_missing: false, overwrite_changed: false });
    expect(c.state.phase).toBe("planned");
    expect(c.planIsCurrent([1])).toBe(true);

    await c.forget("D:\\Laptop");
    expect(api.forgetMirrorTarget).toHaveBeenCalledWith("D:\\Laptop");
    expect(c.state.targets).toEqual([]);
  });

  it("does not guess a selection that was too large to remember", async () => {
    const api = fakeApi();
    const c = createMirrorController(api, browser);
    await c.refresh(target({ mode: "travel", scope: { too_large: 5000 } }), []);
    expect(api.planMirror).not.toHaveBeenCalled();
    expect(c.state.notice).toMatch(/not remembered/);
    expect(c.state.phase).toBe("idle");
  });
});

describe("Browse…", () => {
  it("is offered only in the desktop app", () => {
    expect(browseVisible(true)).toBe(true);
    expect(browseVisible(false)).toBe(false);
    expect(createMirrorController(fakeApi(), desktop).canBrowse()).toBe(true);
    expect(createMirrorController(fakeApi(), browser).canBrowse()).toBe(false);
  });

  it("fills the folder from the native dialog, and does nothing in a browser tab", async () => {
    const c = createMirrorController(fakeApi(), desktop);
    await c.browse();
    expect(c.state.draft.target).toBe("D:\\Picked");

    const pick = vi.fn(async () => "X:\\nope");
    const b = createMirrorController(fakeApi(), { isDesktop: () => false, pickFolder: pick });
    await b.browse();
    expect(pick).not.toHaveBeenCalled();
    expect(b.state.draft.target).toBe("");
  });

  it("keeps the target when the dialog is cancelled", async () => {
    const c = createMirrorController(fakeApi(), { isDesktop: () => true, pickFolder: async () => null });
    c.state.draft.target = "D:\\Keep";
    await c.browse();
    expect(c.state.draft.target).toBe("D:\\Keep");
  });
});

describe("the controller", () => {
  const ready = () => {
    const api = fakeApi();
    const c = createMirrorController(api, browser);
    c.open(0);
    c.state.draft.target = "D:\\Library";
    return { api, c };
  };

  it("refuses to run without a preview, and an edit voids the preview", async () => {
    const { api, c } = ready();
    expect(await c.run([])).toBeNull();
    expect(c.state.error).toBe("Preview first.");
    expect(api.runMirror).not.toHaveBeenCalled();

    await c.preview([]);
    expect(c.state.phase).toBe("planned");
    expect(c.planIsCurrent([])).toBe(true);
    c.state.draft.overwriteChanged = true;
    expect(c.planIsCurrent([])).toBe(false);
    c.edited();
    expect(c.state.plan).toBeNull();
    expect(await c.run([])).toBeNull();
  });

  it("runs the previewed plan with its fingerprint, then reloads the remembered targets", async () => {
    const { api, c } = ready();
    await c.preview([]);
    const done = await c.run([]);
    expect(done?.complete).toBe(true);
    expect(api.runMirror).toHaveBeenCalledWith({ target: "D:\\Library", mode: "travel", scope: { all: true }, remove_missing: true, overwrite_changed: false }, "fp1", expect.anything());
    expect(c.state.phase).toBe("done");
    expect(c.state.plan).toBeNull();
    expect(api.fetchMirrorTargets).toHaveBeenCalled();
  });

  it("surfaces a refused target and a 409, and drops the preview on a 409", async () => {
    const refused = createMirrorController(fakeApi({ planMirror: vi.fn(async () => Promise.reject(new Error("That folder is inside the library"))) }), browser);
    refused.state.draft.target = "D:\\Library";
    await refused.preview([]);
    expect(refused.state.error).toMatch(/inside the library/);
    expect(refused.state.phase).toBe("idle");

    const api = fakeApi({ runMirror: vi.fn(async () => Promise.reject(new Error("409 the copy would no longer be what was previewed"))) });
    const c = createMirrorController(api, browser);
    c.state.draft.target = "D:\\Library";
    await c.preview([]);
    expect(await c.run([])).toBeNull();
    expect(c.state.error).toMatch(/409/);
    expect(c.state.plan).toBeNull();
    expect(c.state.phase).toBe("idle");
  });

  it("does not preview an invalid draft", async () => {
    const api = fakeApi();
    const c = createMirrorController(api, browser);
    await c.preview([]);
    expect(api.planMirror).not.toHaveBeenCalled();
    expect(c.state.error).toMatch(/Choose the folder/);
  });

  it("opens fresh each time", () => {
    const { c } = ready();
    c.state.error = "old";
    c.state.report = report();
    c.open(2);
    expect(c.state.error).toBeNull();
    expect(c.state.report).toBeNull();
    expect(c.state.draft.scope).toBe("selected");
    expect(c.state.draft.target).toBe("");
  });
});
