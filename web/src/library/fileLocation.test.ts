import { describe, expect, it } from "vitest";

import { bookFolder, formatLocation, libraryShortName } from "./fileLocation";

describe("formatLocation", () => {
  it("joins the folder and the file name", () => {
    expect(formatLocation("Unsorted/dupes", "copy of code_silence_0.pdf", "pdf")).toBe("Unsorted/dupes/copy of code_silence_0.pdf");
  });
  it("gives another format the same stem and its own extension", () => {
    expect(formatLocation("A/B", "Book.pdf", "EPUB")).toBe("A/B/Book.epub");
  });
  it("keeps dots inside the name", () => {
    expect(formatLocation("A", "Vol. 2 - The End.pdf", "pdf")).toBe("A/Vol. 2 - The End.pdf");
  });
  it("is empty when the server gave no file name, and bare without a folder", () => {
    expect(formatLocation("A", null, "pdf")).toBe("");
    expect(formatLocation("", "x.pdf", "pdf")).toBe("x.pdf");
  });
});

describe("libraryShortName", () => {
  it("is the last folder of a path, either slash", () => {
    expect(libraryShortName("D:\\Books\\lib")).toBe("lib");
    expect(libraryShortName("/mnt/nas/Books/")).toBe("Books");
  });
  it("leaves a plain name as it is", () => {
    expect(libraryShortName("scratch")).toBe("scratch");
    expect(libraryShortName("")).toBe("");
  });
});

describe("bookFolder", () => {
  it("prefers the book's own path", () => {
    expect(bookFolder("Ann Lee/Book (1)", "other")).toBe("Ann Lee/Book (1)");
  });
  it("falls back to the file row's folder, then to nothing", () => {
    expect(bookFolder(undefined, "Tech/Rust")).toBe("Tech/Rust");
    expect(bookFolder(undefined, undefined)).toBe("");
  });
});
