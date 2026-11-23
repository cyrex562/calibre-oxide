import { describe, expect, it, vi } from "vitest";

import { COLUMN_PRESETS, columnDraftProblem, createColumn, presetDraft } from "./columnPresets";
import { combinedTemplate, createNamingController, NAMING_EXAMPLE, namingProblem, type NamingApi } from "./naming";

describe("naming validation", () => {
  it("combines folder and file the way the server does", () => {
    expect(combinedTemplate("{author_sort}", "{title}")).toBe("{author_sort}/{title}");
    expect(combinedTemplate("", "{title}")).toBe("{title}");
    expect(combinedTemplate("/Gaming/{series}/", " {title} ")).toBe("Gaming/{series}/{title}");
  });

  it("needs a file template, no slash in it, and paired braces", () => {
    expect(namingProblem("{author_sort}", "{title}")).toBeNull();
    expect(namingProblem("", "{title}")).toBeNull();
    expect(namingProblem("x", "  ")).toMatch(/required/);
    expect(namingProblem("x", "a/{title}")).toMatch(/cannot contain/);
    expect(namingProblem("{series", "{title}")).toMatch(/never closed/);
    expect(namingProblem("x", "title}")).toMatch(/no matching/);
    expect(namingProblem(NAMING_EXAMPLE.folder, NAMING_EXAMPLE.file)).toBeNull();
  });
});

function api(over: Partial<NamingApi> = {}): NamingApi {
  return {
    fetchNaming: vi.fn(async () => ({ folder_template: "{author_sort}", file_template: "{title}", combined: "{author_sort}/{title}", defaults: { folder_template: "{author_sort}", file_template: "{title}" } })),
    saveNaming: vi.fn(async (folder: string, file: string) => ({ folder_template: folder, file_template: file, combined: `${folder}/${file}`, defaults: { folder_template: "{author_sort}", file_template: "{title}" } })),
    previewNaming: vi.fn(async () => ({ previews: [{ book_id: 1, title: "T", folder: "A", file: "T.pdf", path: "A/T.pdf", error: null }], combined: "" })),
    ...over,
  } as NamingApi;
}

describe("the naming controller", () => {
  it("loads, previews the sample books, and saves an edited convention", async () => {
    const a = api();
    const c = createNamingController(a);
    await c.load([7, 8]);
    expect(a.previewNaming).toHaveBeenCalledWith("{author_sort}", "{title}", [7, 8]);
    expect(c.state.previews[0].path).toBe("A/T.pdf");
    expect(c.dirty()).toBe(false);

    c.useExample();
    expect(c.dirty()).toBe(true);
    await c.preview();
    expect(a.previewNaming).toHaveBeenLastCalledWith(NAMING_EXAMPLE.folder, NAMING_EXAMPLE.file, [7, 8]);
    expect(await c.save()).toBe(true);
    expect(a.saveNaming).toHaveBeenCalledWith(NAMING_EXAMPLE.folder, NAMING_EXAMPLE.file);
    expect(c.dirty()).toBe(false);
    expect(c.state.note).toMatch(/Saved/);
  });

  it("does not preview or save an invalid template, and shows the problem", async () => {
    const a = api();
    const c = createNamingController(a);
    await c.load();
    (a.previewNaming as ReturnType<typeof vi.fn>).mockClear();
    c.state.file = "";
    await c.preview();
    expect(a.previewNaming).not.toHaveBeenCalled();
    expect(c.state.error).toMatch(/required/);
    expect(await c.save()).toBe(false);
    expect(a.saveNaming).not.toHaveBeenCalled();
  });

  it("shows a server error and drops an answer to an older edit", async () => {
    let release: (v: never) => void = () => {};
    const slow = new Promise<never>((r) => (release = r));
    const a = api({ previewNaming: vi.fn().mockReturnValueOnce(slow).mockResolvedValueOnce({ previews: [{ book_id: 2, title: "N", folder: "", file: "N.pdf", path: "N.pdf", error: null }], combined: "" }) as never });
    const c = createNamingController(a);
    const first = c.preview();
    await c.preview();
    release({ previews: [{ book_id: 1, title: "OLD", folder: "", file: "", path: "OLD", error: null }], combined: "" } as never);
    await first;
    expect(c.state.previews[0].title).toBe("N");

    const failing = createNamingController(api({ previewNaming: vi.fn(async () => Promise.reject(new Error("bad template"))) as never }));
    await failing.preview();
    expect(failing.state.error).toBe("bad template");
  });
});

describe("custom column presets", () => {
  it("define edition, game line and volume with existing column types", () => {
    expect(COLUMN_PRESETS.map((p) => [p.label, p.datatype])).toEqual([["edition", "text"], ["game_line", "text"], ["volume", "int"]]);
    expect(presetDraft(COLUMN_PRESETS[1])).toEqual({ label: "game_line", name: "Game line", datatype: "text" });
  });

  it("creates the column with the edited name", async () => {
    const addCustomColumn = vi.fn(async () => 3);
    const draft = { ...presetDraft(COLUMN_PRESETS[0]), name: " Book edition " };
    expect(await createColumn(draft, { addCustomColumn })).toBe(3);
    expect(addCustomColumn).toHaveBeenCalledWith("edition", "Book edition", "text");
  });

  it("refuses a bad or existing label", () => {
    const d = presetDraft(COLUMN_PRESETS[0]);
    expect(columnDraftProblem(d, [])).toBeNull();
    expect(columnDraftProblem(d, ["edition"])).toMatch(/already/);
    expect(columnDraftProblem({ ...d, label: "Bad Label" }, [])).toMatch(/lower case/);
    expect(columnDraftProblem({ ...d, name: " " }, [])).toMatch(/display name/);
  });
});
