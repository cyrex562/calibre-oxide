// Where a book's files are, for the details panel's Files section (C21).
//
// The server's per-book row (`/ajax/books?files=1`) gives the folder relative to the library
// root and the file name of the first format ("stem.ext"). Every format of a book normally
// shares the stem, so another format's file is that stem with its own extension.

/** `folder/stem.fmt` for one format, or "" when the server did not say. */
export function formatLocation(folder: string | null | undefined, fileName: string | null | undefined, fmt: string): string {
  if (!fileName) return "";
  const dot = fileName.lastIndexOf(".");
  const stem = dot > 0 ? fileName.slice(0, dot) : fileName;
  const name = `${stem}.${fmt.toLowerCase()}`;
  return folder ? `${folder.replace(/[\\/]+$/, "")}/${name}` : name;
}

/** The book's folder: the one on the book itself, else the one in its file row. */
export function bookFolder(path: unknown, rowFolder: string | null | undefined): string {
  if (typeof path === "string" && path !== "") return path;
  return rowFolder ?? "";
}

/**
 * A library's name for a button: the last folder of its path ("D:\Books\lib" is "lib"). The full
 * path belongs in the tooltip; read aloud it is a hundred characters to get to the one that matters.
 */
export function libraryShortName(name: string): string {
  const parts = name.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : name;
}
