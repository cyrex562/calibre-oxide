import { describe, expect, it } from "vitest";

import {
  clamp,
  DETAILS_MAX,
  DETAILS_MIN,
  LAYOUT_DEFAULTS,
  parseLayout,
  resizedWidth,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
} from "./layout";

describe("clamp", () => {
  it("keeps a value inside its bounds", () => {
    expect(clamp(5, 10, 20)).toBe(10);
    expect(clamp(25, 10, 20)).toBe(20);
    expect(clamp(15, 10, 20)).toBe(15);
  });

  it("falls back to the low bound for a non-number", () => {
    expect(clamp(Number.NaN, 10, 20)).toBe(10);
    expect(clamp(Number.POSITIVE_INFINITY, 10, 20)).toBe(10);
  });
});

describe("resizedWidth", () => {
  it("grows the sidebar as the pointer moves right", () => {
    expect(resizedWidth("sidebar", 240, 500, 560)).toBe(300);
  });

  it("shrinks the sidebar as the pointer moves left", () => {
    expect(resizedWidth("sidebar", 240, 500, 440)).toBe(180);
  });

  // The details panel is on the far side of its handle, so the same
  // rightward movement must make it *narrower*. This is the assertion
  // that catches the delta being applied the wrong way round.
  it("shrinks the details panel as the pointer moves right", () => {
    expect(resizedWidth("details", 360, 500, 560)).toBe(300);
  });

  it("grows the details panel as the pointer moves left", () => {
    expect(resizedWidth("details", 360, 500, 440)).toBe(420);
  });

  it("will not drag a panel past its bounds", () => {
    expect(resizedWidth("sidebar", 240, 500, 0)).toBe(SIDEBAR_MIN);
    expect(resizedWidth("sidebar", 240, 500, 5000)).toBe(SIDEBAR_MAX);
    expect(resizedWidth("details", 360, 500, 5000)).toBe(DETAILS_MIN);
    expect(resizedWidth("details", 360, 500, -5000)).toBe(DETAILS_MAX);
  });
});

describe("parseLayout", () => {
  it("returns the defaults when nothing is stored", () => {
    expect(parseLayout(null)).toEqual(LAYOUT_DEFAULTS);
  });

  it("returns the defaults for malformed JSON rather than throwing", () => {
    expect(parseLayout("{not json")).toEqual(LAYOUT_DEFAULTS);
    expect(parseLayout("null")).toEqual(LAYOUT_DEFAULTS);
    expect(parseLayout('"a string"')).toEqual(LAYOUT_DEFAULTS);
  });

  it("round-trips a well-formed blob", () => {
    const stored = JSON.stringify({ sidebarVisible: false, detailsVisible: true, sidebarWidth: 300, detailsWidth: 400 });
    expect(parseLayout(stored)).toEqual({ sidebarVisible: false, detailsVisible: true, sidebarWidth: 300, detailsWidth: 400 });
  });

  // A width saved by a build with wider bounds must not restore a panel
  // bigger than the window, where its splitter is off-screen and the
  // panel can never be dragged back.
  it("clamps a stored width that is out of range", () => {
    const stored = JSON.stringify({ sidebarWidth: 9999, detailsWidth: 1 });
    const out = parseLayout(stored);
    expect(out.sidebarWidth).toBe(SIDEBAR_MAX);
    expect(out.detailsWidth).toBe(DETAILS_MIN);
  });

  it("falls back per field, keeping the good ones", () => {
    const stored = JSON.stringify({ sidebarVisible: "yes", sidebarWidth: 300 });
    const out = parseLayout(stored);
    expect(out.sidebarVisible).toBe(LAYOUT_DEFAULTS.sidebarVisible);
    expect(out.sidebarWidth).toBe(300);
    expect(out.detailsWidth).toBe(LAYOUT_DEFAULTS.detailsWidth);
  });
});
