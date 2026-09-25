import { describe, expect, it } from "vitest";

import {
  readSavedChoice, readVlChoice, savedMenuItems, SAVED_MANAGE, VL_MANAGE, VL_NONE, vlMenuItems,
} from "./searchMenus";

describe("vlMenuItems", () => {
  it("always leads with All books and ends with manage", () => {
    const items = vlMenuItems(["Unread", "Comics"], "");
    expect(items[0].id).toBe(VL_NONE);
    expect(items[items.length - 1].id).toBe(VL_MANAGE);
  });

  it("ticks the active library, and only it", () => {
    const ticked = vlMenuItems(["Unread", "Comics"], "Comics").filter((i) => i.checked);
    expect(ticked.map((i) => i.label)).toEqual(["Comics"]);
  });

  it("ticks All books when nothing is active", () => {
    expect(vlMenuItems(["Unread"], "").find((i) => i.checked)!.id).toBe(VL_NONE);
  });

  it("offers manage even with no libraries yet", () => {
    expect(vlMenuItems([], "").map((i) => i.id)).toEqual([VL_NONE, VL_MANAGE]);
  });
});

describe("readVlChoice", () => {
  it("reads a name back exactly", () => {
    expect(readVlChoice("vl:Science Fiction")).toEqual({ kind: "apply", name: "Science Fiction" });
  });

  it("maps the none sentinel to the empty selection", () => {
    expect(readVlChoice(VL_NONE)).toEqual({ kind: "apply", name: "" });
  });

  it("maps the manage sentinel to manage", () => {
    expect(readVlChoice(VL_MANAGE)).toEqual({ kind: "manage" });
  });

  // A library called "__manage__" must not silently open the dialog:
  // the sentinel is the *whole* id, not a suffix.
  it("does not mistake a library named like a sentinel", () => {
    expect(readVlChoice("vl:__manage__x")).toEqual({ kind: "apply", name: "__manage__x" });
  });

  it("round-trips every name the menu can produce", () => {
    const names = ["Unread", "A, B", "vl:weird", "__none__"];
    for (const item of vlMenuItems(names, "")) {
      if (item.id === VL_NONE || item.id === VL_MANAGE) continue;
      expect(readVlChoice(item.id)).toEqual({ kind: "apply", name: item.label });
    }
  });
});

describe("savedMenuItems", () => {
  it("offers manage even when there are none", () => {
    expect(savedMenuItems([]).map((i) => i.id)).toEqual([SAVED_MANAGE]);
  });

  it("separates manage from the list only when there is a list", () => {
    const none = savedMenuItems([]);
    const some = savedMenuItems(["Recent"]);
    expect(none[none.length - 1].startsGroup).toBe(false);
    expect(some[some.length - 1].startsGroup).toBe(true);
  });
});

describe("readSavedChoice", () => {
  it("reads a name back exactly", () => {
    expect(readSavedChoice("ss:To read")).toEqual({ kind: "apply", name: "To read" });
  });

  it("maps the manage sentinel", () => {
    expect(readSavedChoice(SAVED_MANAGE)).toEqual({ kind: "manage" });
  });

  it("round-trips names containing a colon", () => {
    expect(readSavedChoice("ss:tag:sf")).toEqual({ kind: "apply", name: "tag:sf" });
  });
});
