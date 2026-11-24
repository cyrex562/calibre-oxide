import { describe, expect, it } from "vitest";

import { convertTargets, defaultTarget } from "./convertTargets";

const OUTPUTS = ["azw3", "docx", "epub", "lit", "lrf", "markdown", "md", "pdf", "text", "txt"];

describe("convertTargets", () => {
  it("leaves out the source format, the obsolete formats and the alias names", () => {
    expect(convertTargets(OUTPUTS, "pdf")).toEqual(["azw3", "docx", "epub", "markdown", "txt"]);
  });
  it("matches formats whatever their case", () => {
    expect(convertTargets(["EPUB", "PDF", "MD", "MARKDOWN"], "Pdf")).toEqual(["EPUB", "MARKDOWN"]);
  });
  it("keeps an alias when the name it stands for is not offered", () => {
    expect(convertTargets(["md", "epub"], "pdf")).toEqual(["md", "epub"]);
  });
});

describe("defaultTarget", () => {
  it("is the first format the book does not have yet", () => {
    expect(defaultTarget(["epub", "azw3"], ["pdf", "epub"])).toBe("azw3");
  });
  it("falls back to the first, and to nothing", () => {
    expect(defaultTarget(["epub"], ["epub"])).toBe("epub");
    expect(defaultTarget([], [])).toBe("");
  });
});
