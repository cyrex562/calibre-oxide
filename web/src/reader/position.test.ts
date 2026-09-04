import { describe, expect, it } from "vitest";
import { decodePosition, encodePosition } from "./position";

describe("position encode/decode", () => {
  it("round-trips a spine index and fragment", () => {
    const encoded = encodePosition({ spineIndex: 3, frag: "section-2" });
    expect(decodePosition(encoded)).toEqual({ spineIndex: 3, frag: "section-2" });
  });

  it("round-trips an empty fragment", () => {
    const encoded = encodePosition({ spineIndex: 0, frag: "" });
    expect(decodePosition(encoded)).toEqual({ spineIndex: 0, frag: "" });
  });

  it("returns null for an unrelated cfi string", () => {
    expect(decodePosition("epubcfi(/6/4!/2/1:0)")).toBeNull();
  });

  it("returns null for null/undefined input", () => {
    expect(decodePosition(null)).toBeNull();
    expect(decodePosition(undefined)).toBeNull();
  });
});
