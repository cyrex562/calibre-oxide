import { describe, expect, it } from "vitest";

import { activeRules, ALLOWED_COLORS, colorForBook, colorFromResult, type ColoringRule } from "./coloringRules";

const rule = (name: string, template = "program: 'red'", enabled = true): ColoringRule => ({ name, template, enabled });

describe("colorFromResult", () => {
  it("accepts every named colour", () => {
    for (const c of ALLOWED_COLORS) expect(colorFromResult(c), c).toBe(c);
  });

  it("is case- and whitespace-insensitive", () => {
    expect(colorFromResult("  RED  ")).toBe("red");
  });

  it("accepts short and long hex", () => {
    expect(colorFromResult("#f00")).toBe("#f00");
    expect(colorFromResult("#ff0000")).toBe("#ff0000");
  });

  it("treats an empty result as no match", () => {
    expect(colorFromResult("")).toBeNull();
    expect(colorFromResult("   ")).toBeNull();
    expect(colorFromResult(undefined)).toBeNull();
    expect(colorFromResult(null)).toBeNull();
  });

  // The value goes into a `style` attribute and a template is
  // user-authored text that can return anything. An allowlist means
  // nothing else gets through; a regex over CSS colour syntax could
  // not promise that.
  it("refuses anything that is not plainly a colour", () => {
    expect(colorFromResult("red; background: url(http://evil)")).toBeNull();
    expect(colorFromResult("expression(alert(1))")).toBeNull();
    expect(colorFromResult("rgb(255,0,0)")).toBeNull();
    expect(colorFromResult("var(--x)")).toBeNull();
    expect(colorFromResult("#ff")).toBeNull();
    expect(colorFromResult("#gggggg")).toBeNull();
    expect(colorFromResult("notacolour")).toBeNull();
  });
});

describe("colorForBook", () => {
  // The list order in settings *is* the precedence, which is what a
  // user reordering that list expects.
  it("takes the first matching enabled rule", () => {
    const rules = [rule("a"), rule("b")];
    expect(colorForBook(rules, ["blue", "green"])).toBe("blue");
  });

  it("skips disabled rules even when they match", () => {
    const rules = [rule("a", "program: 'red'", false), rule("b")];
    expect(colorForBook(rules, ["blue", "green"])).toBe("green");
  });

  it("falls through a rule that did not match", () => {
    expect(colorForBook([rule("a"), rule("b")], ["", "green"])).toBe("green");
  });

  // A rule whose output cannot be shown should fall through rather
  // than swallowing the row's colour entirely.
  it("falls through a rule that returned something unusable", () => {
    expect(colorForBook([rule("a"), rule("b")], ["url(evil)", "green"])).toBe("green");
  });

  it("returns null when nothing matches", () => {
    expect(colorForBook([rule("a")], [""])).toBeNull();
    expect(colorForBook([], [])).toBeNull();
  });

  it("copes with fewer results than rules", () => {
    expect(colorForBook([rule("a"), rule("b")], ["red"])).toBe("red");
    expect(colorForBook([rule("a"), rule("b")], [])).toBeNull();
  });
});

describe("activeRules", () => {
  it("drops disabled and blank rules", () => {
    const rules = [rule("on"), rule("off", "program: 'x'", false), rule("blank", "   ")];
    expect(activeRules(rules).map((r) => r.name)).toEqual(["on"]);
  });
});
