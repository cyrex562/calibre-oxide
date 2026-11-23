import { describe, expect, it, vi } from "vitest";

import { addChosenDuplicates, describeImport, importFiles, type AddFn } from "./importFiles";
import type { AddBookResult } from "./types";

const file = (name: string) => new File(["x"], name);
const added = (): AddBookResult => ({ title: "t", authors: [], languages: [], filename: "f", id: "1", book_id: 7 });
const dup = (title: string): AddBookResult => ({ title, authors: [], languages: [], filename: "f", id: "1", duplicates: [{ title, authors: ["Ann Lee", "Bo Chen"] }] });

describe("importFiles", () => {
  it("collects the duplicates of the whole import instead of asking for each one", async () => {
    const add = vi.fn<AddFn>(async (f) => (f.name.startsWith("dup") ? dup("Dune") : added()));
    const out = await importFiles([file("a.pdf"), file("dup1.pdf"), file("b.pdf"), file("dup2.pdf")], add, "ask");
    expect(out.added).toBe(2);
    expect(out.duplicates.map((d) => d.file.name)).toEqual(["dup1.pdf", "dup2.pdf"]);
    expect(out.duplicates[0].existing).toEqual(["Dune (Ann Lee & Bo Chen)"]);
    // Nothing was uploaded a second time while the user has not decided.
    expect(add).toHaveBeenCalledTimes(4);
    expect(add.mock.calls.every(([, flag]) => flag === false)).toBe(true);
  });

  it("skips duplicates without any prompt in skip mode, and adds them straight away in add mode", async () => {
    const add = vi.fn<AddFn>(async (_f, flag) => (flag ? added() : dup("Dune")));
    const skipped = await importFiles([file("a.pdf")], add, "skip");
    expect(skipped).toMatchObject({ added: 0, skipped: 1, duplicates: [] });
    const all = await importFiles([file("a.pdf")], add, "add");
    expect(all).toMatchObject({ added: 1, skipped: 0, duplicates: [] });
  });

  it("reports a failing file and carries on with the rest", async () => {
    const add = vi.fn<AddFn>(async (f) => {
      if (f.name === "bad.pdf") throw new Error("unreadable");
      return added();
    });
    const out = await importFiles([file("bad.pdf"), file("ok.pdf")], add, "ask");
    expect(out.errors).toEqual(["bad.pdf: unreadable"]);
    expect(out.added).toBe(1);
  });
});

describe("addChosenDuplicates", () => {
  it("uploads only the chosen ones, with the add-duplicates flag", async () => {
    const add = vi.fn<AddFn>(async () => added());
    const result = await addChosenDuplicates([{ file: file("a.pdf"), existing: [] }], add);
    expect(result).toEqual({ added: 1, errors: [] });
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ name: "a.pdf" }), true);
  });

  it("keeps going after one fails", async () => {
    const add = vi.fn<AddFn>(async (f) => {
      if (f.name === "a.pdf") throw new Error("nope");
      return added();
    });
    const result = await addChosenDuplicates([{ file: file("a.pdf"), existing: [] }, { file: file("b.pdf"), existing: [] }], add);
    expect(result.added).toBe(1);
    expect(result.errors).toEqual(["a.pdf: nope"]);
  });
});

describe("describeImport", () => {
  it("says how many went in, were skipped, failed", () => {
    expect(describeImport(5, 3, 1, 1)).toBe("Added 3 of 5 book(s), 1 skipped, 1 failed.");
    expect(describeImport(2, 2, 0, 0)).toBe("Added 2 of 2 book(s).");
  });
});
