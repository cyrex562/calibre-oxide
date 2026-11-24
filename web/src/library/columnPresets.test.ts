import { describe, expect, it } from "vitest";

import { columnDraftProblem, COLUMN_PRESETS, labelFromName } from "./columnPresets";

describe("labelFromName", () => {
  it("turns what a person types into a label the server accepts", () => {
    expect(labelFromName("Scan Quality")).toBe("scan_quality");
    expect(labelFromName("  Read status! ")).toBe("read_status");
    expect(labelFromName("Édition (2nd)")).toBe("edition_2nd");
  });
  it("is empty for a name with no letters or digits", () => {
    expect(labelFromName("…")).toBe("");
  });
  it("makes labels that columnDraftProblem accepts, for every preset's name", () => {
    for (const p of COLUMN_PRESETS) {
      expect(labelFromName(p.name) === p.label || p.id === "game_line").toBe(true);
      expect(columnDraftProblem({ label: labelFromName(p.name), name: p.name, datatype: p.datatype }, [])).toBeNull();
    }
  });
});

describe("presets for sorting out a collection", () => {
  it("offers read status, source and scan quality", () => {
    const ids = COLUMN_PRESETS.map((p) => p.id);
    expect(ids).toEqual(expect.arrayContaining(["read_status", "source", "scan_quality"]));
  });
});
