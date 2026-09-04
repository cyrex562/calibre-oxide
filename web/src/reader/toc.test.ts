import { describe, expect, it } from "vitest";
import { flattenToc } from "./toc";
import type { TocNode } from "./types";

describe("flattenToc", () => {
  it("flattens a nested TOC in document order with depth", () => {
    const root: TocNode = {
      title: "",
      dest: null,
      frag: null,
      id: 0,
      children: [
        {
          title: "Chapter One",
          dest: "chap1.xhtml",
          frag: null,
          id: 1,
          children: [{ title: "Section 1.1", dest: "chap1.xhtml", frag: "s1", id: 2, children: [] }],
        },
        { title: "Chapter Two", dest: "chap2.xhtml", frag: null, id: 3, children: [] },
      ],
    };
    const flat = flattenToc(root);
    expect(flat).toEqual([
      { title: "Chapter One", dest: "chap1.xhtml", frag: null, depth: 1 },
      { title: "Section 1.1", dest: "chap1.xhtml", frag: "s1", depth: 2 },
      { title: "Chapter Two", dest: "chap2.xhtml", frag: null, depth: 1 },
    ]);
  });

  it("omits the synthetic root container itself", () => {
    const root: TocNode = { title: "", dest: null, frag: null, id: 0, children: [] };
    expect(flattenToc(root)).toEqual([]);
  });
});
