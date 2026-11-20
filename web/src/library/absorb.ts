// "Calibre libraries inside this library": bringing a calibre library that sits
// in a subfolder in, in place.
//
// What the server does is described in crates/calibre_db/src/absorb.rs: each
// book of the nested `metadata.db` is registered where its files already are
// (nothing is moved or copied), with its metadata and custom columns, and the
// nested database is renamed to `metadata.db.absorbed-by-calibre-oxide` when
// every book is in. This is the part the panel needs: the types, and the words
// for what came of it.

export interface AbsorbableLibrary {
  /** Relative to the library, `/`-separated. */
  path: string;
  /** `available`: has a live `metadata.db`. `absorbed`: its books are in. */
  state: "available" | "absorbed";
  /** Books in the nested database (or, once absorbed, in this library under the folder). */
  books: number;
  /** For `available`: how many of them are in already (a partly done absorb). */
  already_absorbed: number;
  problem: string | null;
}

export interface AbsorbableList {
  libraries: AbsorbableLibrary[];
  /** False when the search ran out of time or a folder could not be read. */
  conclusive: boolean;
}

export interface AbsorbProblem {
  source_id: number;
  title: string;
  error: string;
}

export interface AbsorbReport {
  folder: string;
  source_books: number;
  absorbed: number;
  already_absorbed: number;
  formats: number;
  covers: number;
  annotations: number;
  custom_values: number;
  failed: AbsorbProblem[];
  failed_count: number;
  missing_files: string[];
  missing_count: number;
  uuid_remapped: { source_uuid: string; new_uuid: string; book_id: number }[];
  custom_columns_created: string[];
  custom_column_conflicts: string[];
  warnings: string[];
  cancelled: boolean;
  /** The nested `metadata.db` was renamed, so the folder is now an ordinary one. */
  renamed: boolean;
  not_renamed_because: string | null;
  not_carried: string[];
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (count: number, word: string) => `${n(count)} ${word}${count === 1 ? "" : "s"}`;

/** What a nested library's row says: its size and where it stands. */
export function describeAbsorbable(lib: AbsorbableLibrary): string {
  if (lib.problem) return `Could not be read: ${lib.problem}`;
  if (lib.state === "absorbed") return `${plural(lib.books, "book")} in this library`;
  if (lib.already_absorbed > 0) return `${plural(lib.books, "book")}, ${n(lib.already_absorbed)} already in — partly absorbed`;
  return plural(lib.books, "book");
}

/** The outcome as short sentences, most important first. */
export function describeAbsorb(r: AbsorbReport): string[] {
  const lines: string[] = [];
  if (r.absorbed === 0 && r.already_absorbed === r.source_books && !r.cancelled) lines.push("Nothing new: every book was in already.");
  else lines.push(`Brought in ${plural(r.absorbed, "book")} (${plural(r.formats, "file")}${r.covers ? `, ${plural(r.covers, "cover")}` : ""}${r.custom_values ? `, ${plural(r.custom_values, "custom value")}` : ""}).`);
  if (r.already_absorbed > 0 && r.absorbed > 0) lines.push(`${plural(r.already_absorbed, "book")} had been brought in before.`);
  if (r.cancelled) lines.push("Stopped early. Absorb the library again to carry on where it left off.");
  if (r.renamed) lines.push("Done. Its metadata.db was renamed to metadata.db.absorbed-by-calibre-oxide, so a scan treats the folder as ordinary. Rename it back to undo.");
  else if (r.not_renamed_because) lines.push(r.not_renamed_because.charAt(0).toUpperCase() + r.not_renamed_because.slice(1) + (r.not_renamed_because.endsWith(".") ? "" : "."));
  if (r.custom_columns_created.length) lines.push(`Created ${plural(r.custom_columns_created.length, "custom column")}: ${r.custom_columns_created.map((c) => `#${c}`).join(", ")}.`);
  if (r.uuid_remapped.length) lines.push(`${plural(r.uuid_remapped.length, "book")} already had its uuid here and got a new one.`);
  if (r.missing_count) lines.push(`${plural(r.missing_count, "file")} listed by calibre ${r.missing_count === 1 ? "is" : "are"} missing on disk.`);
  if (r.failed_count) lines.push(`${plural(r.failed_count, "book")} could not be brought in.`);
  return lines;
}

/** Every problem the user may need to act on. */
export function absorbProblems(r: AbsorbReport): string[] {
  return [
    ...r.custom_column_conflicts.map((c) => `Custom column ${c}; its values were not carried.`),
    ...r.failed.map((f) => `${f.title || `book ${f.source_id}`}: ${f.error}`),
    ...r.missing_files.map((m) => `Missing: ${m}`),
    ...r.warnings,
  ];
}
