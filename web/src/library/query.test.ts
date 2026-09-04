import { describe, expect, it } from "vitest";
import { categoryItemToQuery } from "./query";

describe("categoryItemToQuery", () => {
  it("builds an exact-match field clause", () => {
    expect(categoryItemToQuery("authors", "J. R. R. Tolkien")).toBe('authors:"=J. R. R. Tolkien"');
  });

  it("escapes an internal double quote so it can't break out of the clause", () => {
    expect(categoryItemToQuery("tags", 'Sci-Fi "Classics"')).toBe('tags:"=Sci-Fi \\"Classics\\""');
  });
});
