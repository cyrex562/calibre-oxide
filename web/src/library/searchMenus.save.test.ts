// "Save this search" and "Make a virtual library from this search" in the two menus (C19).

import { describe, expect, it } from "vitest";

import { readSavedChoice, readVlChoice, SAVED_MANAGE, SAVED_SAVE, savedMenuItems, VL_FROM_SEARCH, VL_MANAGE, vlMenuItems } from "./searchMenus";

describe("saved searches menu", () => {
  it("leads with Save this search when a search is on screen", () => {
    const items = savedMenuItems(["Travel"], "tags:travel");
    expect(items.map((i) => i.id)).toEqual([SAVED_SAVE, "ss:Travel", SAVED_MANAGE]);
    expect(readSavedChoice(SAVED_SAVE)).toEqual({ kind: "save" });
  });
  it("does not offer it with no search", () => {
    expect(savedMenuItems(["Travel"]).map((i) => i.id)).toEqual(["ss:Travel", SAVED_MANAGE]);
    expect(savedMenuItems([], "   ").map((i) => i.id)).toEqual([SAVED_MANAGE]);
  });
  it("still reads a saved search called anything back by name", () => {
    expect(readSavedChoice("ss:Save this search")).toEqual({ kind: "apply", name: "Save this search" });
  });
});

describe("virtual library menu", () => {
  it("offers a virtual library from the search on screen, before Manage", () => {
    const ids = vlMenuItems(["Unread"], "", "tags:travel").map((i) => i.id);
    expect(ids).toEqual(["vl:__none__", "vl:Unread", VL_FROM_SEARCH, VL_MANAGE]);
    expect(readVlChoice(VL_FROM_SEARCH)).toEqual({ kind: "from-search" });
  });
  it("leaves it out with no search", () => {
    expect(vlMenuItems(["Unread"], "").map((i) => i.id)).not.toContain(VL_FROM_SEARCH);
  });
});
