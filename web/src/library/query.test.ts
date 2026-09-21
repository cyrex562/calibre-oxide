import { describe, expect, it } from "vitest";
import { categoryItemToQuery, similarBooksQuery } from "./query";

describe("categoryItemToQuery", () => {
  it("builds an exact-match field clause", () => {
    expect(categoryItemToQuery("authors", "J. R. R. Tolkien")).toBe('authors:"=J. R. R. Tolkien"');
  });

  it("escapes an internal double quote so it can't break out of the clause", () => {
    expect(categoryItemToQuery("tags", 'Sci-Fi "Classics"')).toBe('tags:"=Sci-Fi \\"Classics\\""');
  });
});

describe("similarBooksQuery", () => {
  const book = { id: 7, authors: ["Ann Lee", "Bo Fox"], tags: ["sci-fi"], series: "Foundation", publisher: "Acme" };

  it("ORs every clause and excludes the book itself", () => {
    // The book trivially matches every clause; a similar-books list
    // led by the book you came from is just noise.
    const q = similarBooksQuery(book, { authors: true });
    expect(q).toBe('(authors:"=Ann Lee" or authors:"=Bo Fox") and not id:7');
  });

  it("does not wrap a single clause in parentheses", () => {
    expect(similarBooksQuery(book, { series: true })).toBe('series:"=Foundation" and not id:7');
  });

  it("combines every requested basis", () => {
    const q = similarBooksQuery({ id: 1, authors: ["A"], tags: ["t"], series: "S", publisher: "P" }, { authors: true, tags: true, series: true, publisher: true });
    expect(q).toBe('(authors:"=A" or tags:"=t" or series:"=S" or publisher:"=P") and not id:1');
  });

  it("ignores a basis the caller did not ask for", () => {
    expect(similarBooksQuery(book, { tags: true })).toBe('tags:"=sci-fi" and not id:7');
  });

  // Distinguishing "nothing to match on" from "nothing matched" is
  // the whole reason this returns null -- the two look identical to a
  // user staring at an empty result list.
  it("returns null when the book has nothing to match on", () => {
    expect(similarBooksQuery({ id: 1 }, { authors: true, tags: true, series: true, publisher: true })).toBeNull();
    expect(similarBooksQuery({ id: 1, authors: [], tags: [], series: null, publisher: null }, { authors: true })).toBeNull();
  });

  it("returns null when no basis is selected at all", () => {
    expect(similarBooksQuery(book, {})).toBeNull();
  });

  it("skips empty values inside a list", () => {
    expect(similarBooksQuery({ id: 2, authors: ["", "Real Name"] }, { authors: true })).toBe('authors:"=Real Name" and not id:2');
  });

  it("escapes quotes so a name cannot break out of its clause", () => {
    const q = similarBooksQuery({ id: 3, publisher: 'Acme "Books"' }, { publisher: true });
    expect(q).toBe('publisher:"=Acme \\"Books\\"" and not id:3');
  });
});
