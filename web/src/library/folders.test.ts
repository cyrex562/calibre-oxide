import { describe, expect, it, vi } from "vitest";

import type { LibraryFolder, OrganizePlan, OrganizeReport } from "./api";
import {
  ancestorsOf, BOOKS_DRAG_TYPE, buildFolderTree, createFolderFlow, dragIds, folderNameProblem, folderQuery, hasBooksDrag,
  readBooksDrag, renamedPath, setBooksDrag, toggleFolder, visibleFolderRows, type FolderApi,
} from "./folders";

const f = (path: string, books: number, books_here = 0, empty = false): LibraryFolder => ({ path, books, books_here, empty });

const FOLDERS = [f("Gaming", 4), f("Gaming/RPG", 4), f("Gaming/RPG/Rift", 1, 1), f("Gaming/RPG/Rifts", 3, 2), f("Gaming/RPG/Rifts/Sub", 1, 1), f("Later", 0, 0, true)];

describe("the folder tree", () => {
  it("nests the flat list and keeps the counts", () => {
    const tree = buildFolderTree(FOLDERS);
    expect(tree.map((n) => [n.name, n.books])).toEqual([["Gaming", 4], ["Later", 0]]);
    const rpg = tree[0].children[0];
    expect(rpg.path).toBe("Gaming/RPG");
    expect(rpg.children.map((n) => [n.name, n.books, n.booksHere])).toEqual([["Rift", 1, 1], ["Rifts", 3, 2]]);
    expect(rpg.children[1].children[0].path).toBe("Gaming/RPG/Rifts/Sub");
    expect(tree[1].empty).toBe(true);
  });

  it("makes up a missing parent rather than dropping the folder", () => {
    const tree = buildFolderTree([f("A/B/C", 1, 1)]);
    expect(tree[0].children[0].children[0].path).toBe("A/B/C");
  });

  it("draws children only of expanded folders, and expansion toggles", () => {
    const tree = buildFolderTree(FOLDERS);
    expect(visibleFolderRows(tree, new Set()).map((r) => r.node.path)).toEqual(["Gaming", "Later"]);
    let open = toggleFolder(new Set(), "Gaming");
    open = toggleFolder(open, "Gaming/RPG");
    expect(visibleFolderRows(tree, open).map((r) => [r.node.path, r.depth])).toEqual([["Gaming", 0], ["Gaming/RPG", 1], ["Gaming/RPG/Rift", 2], ["Gaming/RPG/Rifts", 2], ["Later", 0]]);
    expect(toggleFolder(open, "Gaming").has("Gaming")).toBe(false);
    expect(ancestorsOf("Gaming/RPG/Rifts")).toEqual(["Gaming", "Gaming/RPG"]);
  });
});

describe("folder search and names", () => {
  it("quotes the folder for the search language", () => {
    expect(folderQuery("Gaming/RPG")).toBe('folder:"Gaming/RPG"');
    expect(folderQuery("Gaming\\RPG\\")).toBe('folder:"Gaming/RPG"');
    expect(folderQuery('A "quoted" one')).toBe('folder:"A \\"quoted\\" one"');
  });

  it("checks a folder name", () => {
    expect(folderNameProblem("Rifts")).toBeNull();
    for (const bad of ["", "  ", "a/b", "a\\b", "..", "a:b", "x*", "end."]) expect(folderNameProblem(bad), bad).not.toBeNull();
    expect(renamedPath("Gaming/RPG/Rifts", "Rifts RPG")).toBe("Gaming/RPG/Rifts RPG");
    expect(renamedPath("Top", "Bottom")).toBe("Bottom");
  });
});

describe("the drag payload", () => {
  const data = () => {
    const store: Record<string, string> = {};
    return { store, setData: (t: string, v: string) => void (store[t] = v), getData: (t: string) => store[t] ?? "", types: [] as string[], effectAllowed: "" };
  };

  it("carries the selection when the dragged row is selected, else the row alone", () => {
    expect(dragIds(2, new Set([1, 2, 3]), true)).toEqual([1, 2, 3]);
    expect(dragIds(9, new Set([1, 2, 3]), true)).toEqual([9]);
    expect(dragIds(2, new Set([1, 2, 3]), false)).toEqual([2]);
  });

  it("round trips the ids under its own type", () => {
    const d = data();
    setBooksDrag(d, [4, 5]);
    expect(d.store[BOOKS_DRAG_TYPE]).toBe("[4,5]");
    expect(d.effectAllowed).toBe("move");
    expect(hasBooksDrag({ types: [BOOKS_DRAG_TYPE, "text/plain"] })).toBe(true);
    expect(hasBooksDrag({ types: ["Files"] })).toBe(false);
    expect(readBooksDrag(d)).toEqual([4, 5]);
    expect(readBooksDrag(data())).toEqual([]);
  });
});

const plan = (books: number, extra: Partial<OrganizePlan> = {}): OrganizePlan & { folder: string; from: string; to: string } => ({ plan_fingerprint: "fp", template: "", books, files: books, conflicts: 0, already_organized: 0, skipped: [], moves: [], folder: "X", from: "A", to: "B", ...extra });
const report: OrganizeReport = { run_id: "org-1", books_planned: 2, books_moved: 2, files_moved: 2, failed: [], cancelled: false };

function fakeApi(over: Partial<FolderApi> = {}): FolderApi {
  return {
    planMoveBooks: vi.fn(async () => plan(2)),
    runMoveBooks: vi.fn(async () => report),
    planRenameFolder: vi.fn(async () => plan(2)),
    runRenameFolder: vi.fn(async () => report),
    undoOrganize: vi.fn(async () => ({ run_id: "org-1", books_restored: 2, files_restored: 2, skipped: [] })),
    ...over,
  } as FolderApi;
}

describe("the move flow: dry run, then run, then refresh", () => {
  it("previews first, runs only after confirm, refreshes, and can undo", async () => {
    const api = fakeApi();
    const changed = vi.fn();
    const flow = createFolderFlow(api, changed);
    expect(await flow.run()).toBe(false); // nothing previewed
    expect(await flow.begin({ kind: "move", ids: [1, 2], folder: "Gaming/RPG" })).toBe(true);
    expect(flow.state.phase).toBe("confirm");
    expect(api.planMoveBooks).toHaveBeenCalledWith([1, 2], "Gaming/RPG");
    expect(api.runMoveBooks).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();

    expect(await flow.run()).toBe(true);
    expect(api.runMoveBooks).toHaveBeenCalledWith([1, 2], "Gaming/RPG", "fp", expect.anything());
    expect(flow.state.phase).toBe("done");
    expect(changed).toHaveBeenCalledTimes(1);

    expect(await flow.undo()).toBe(true);
    expect(api.undoOrganize).toHaveBeenCalledWith("org-1");
    expect(flow.state.undone).toContain("2 books put back");
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("says why when there is nothing to move, and when the server refuses", async () => {
    const same = createFolderFlow(fakeApi({ planMoveBooks: vi.fn(async () => plan(0, { skipped: [{ book_id: 1, reason: "no file" }] })) as never }));
    expect(await same.begin({ kind: "move", ids: [1], folder: "X" })).toBe(false);
    expect(same.state.error).toBe("no file");

    const refused = createFolderFlow(fakeApi({ planRenameFolder: vi.fn(async () => Promise.reject(new Error("a folder named B already exists"))) as never }));
    expect(await refused.begin({ kind: "rename", from: "A", to: "B" })).toBe(false);
    expect(refused.state.error).toBe("a folder named B already exists");
    expect(refused.state.phase).toBe("idle");
  });

  it("renames through the rename route and voids a stale preview", async () => {
    const api = fakeApi({ runRenameFolder: vi.fn(async () => Promise.reject(new Error("the plan is no longer what was previewed"))) as never });
    const flow = createFolderFlow(api);
    await flow.begin({ kind: "rename", from: "A", to: "B" });
    expect(api.planRenameFolder).toHaveBeenCalledWith("A", "B");
    expect(await flow.run()).toBe(false);
    expect(flow.state.plan).toBeNull();
    expect(flow.state.phase).toBe("idle");
  });
});
