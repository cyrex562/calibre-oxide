import { describe, expect, it } from "vitest";

import { activeSection } from "./scrollSpy";

const page = (tops: number[]) => tops.map((top, i) => ({ id: `s${i}`, top }));

describe("activeSection", () => {
  it("is the last section whose top has reached the reading line", () => {
    expect(activeSection(page([-900, -300, 40, 600]), 80)).toBe("s2");
  });
  it("is the first section when none has reached the line yet", () => {
    expect(activeSection(page([120, 500, 900]), 80)).toBe("s0");
  });
  it("does not jump to the last section just because it is on screen", () => {
    // Toolbar (s5) at the top of the window, Email (s6) lower down: Toolbar is the active one.
    expect(activeSection(page([-2000, -1500, -1000, -600, -300, 20, 380]), 80)).toBe("s5");
  });
  it("copes with no sections", () => {
    expect(activeSection([], 80)).toBeNull();
  });
});
