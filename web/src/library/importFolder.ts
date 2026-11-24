// "Add folder": copying a folder tree into the library as a job.
//
// What the server does is described in crates/lexmechanic_db/src/import.rs; this
// is the part the dialog needs: the choices (calibre's two ways of reading a
// folder, and what to do about duplicates), the request, and the words for
// what came of it.

export type ImportMode = "one_book_per_folder" | "multiple_books_per_folder";
export type DuplicatePolicy = "skip_exact_content" | "skip_title_author" | "add_formats_to_existing" | "add_anyway";

export interface ImportFolderRequest {
  /** A folder on the machine running the server. */
  source: string;
  mode: ImportMode;
  policy: DuplicatePolicy;
  recursive: boolean;
  /** Put each book in the folder it came from, instead of calibre's Author/Title (id). */
  keep_structure: boolean;
}

export interface Choice<T extends string> {
  value: T;
  label: string;
  help: string;
}

export const IMPORT_MODES: Choice<ImportMode>[] = [
  {
    value: "multiple_books_per_folder",
    label: "Several books per folder",
    help: "Files with the same name but different extensions (a.epub, a.pdf) are formats of one book; every other file is a book of its own.",
  },
  {
    value: "one_book_per_folder",
    label: "One book per folder",
    help: "Everything in a folder is one book, its formats. A folder with two PDFs gets one of them; the other is listed, not imported.",
  },
];

export const DUPLICATE_POLICIES: Choice<DuplicatePolicy>[] = [
  {
    value: "skip_exact_content",
    label: "Skip files already in the library",
    help: "A file whose contents are identical to one already here (or earlier in this import) is skipped, whatever it is called.",
  },
  {
    value: "skip_title_author",
    label: "Also skip books with the same title and author",
    help: "On top of identical files, a book matching an existing one by title and author is skipped. Books with no author are never matched on title alone.",
  },
  {
    value: "add_formats_to_existing",
    label: "Add new formats to the matching book",
    help: "A book matching an existing one by title and author gives it any format it lacks, instead of becoming a second record. Formats it already has are kept.",
  },
  {
    value: "add_anyway",
    label: "Add everything",
    help: "No duplicate checks: identical files become separate books.",
  },
];

/** Where the copies are put. What each produces is spelled out for the dialog. */
export const IMPORT_LAYOUTS: { keep: boolean; label: string; help: string }[] = [
  {
    keep: false,
    label: "Author / Title (calibre's layout)",
    help: "Each book becomes its own folder: Author/Title (id)/Title - Author.pdf. Your source folders are not kept, but the library can find and sort every book from its metadata.",
  },
  {
    keep: true,
    label: "Keep my folders",
    help: "Each book is copied to the same place under the library: Fiction/SciFi/Dune.pdf in the source becomes Fiction/SciFi/Dune.pdf here. A name already taken gets a number, and nothing is replaced. Organize can still re-file them later.",
  },
];

export const DEFAULT_IMPORT = {
  mode: "multiple_books_per_folder" as ImportMode,
  policy: "skip_exact_content" as DuplicatePolicy,
  recursive: true,
  keepStructure: false,
};

/**
 * Whether `path` is the absolute path of a folder, in the forms the server
 * (any platform) accepts. Only a shape check: whether it exists is for the
 * server to say.
 */
export function validateSource(path: string): string | null {
  const p = path.trim();
  if (p === "") return "Choose the folder to import.";
  const absolute = p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\");
  return absolute ? null : "Give the full path of the folder, starting from the drive or root.";
}

export function importBody(req: ImportFolderRequest): ImportFolderRequest {
  return { source: req.source.trim(), mode: req.mode, policy: req.policy, recursive: req.recursive, keep_structure: req.keep_structure };
}

export interface ImportProblem {
  path: string;
  error: string;
}

export interface ImportReport {
  source: string;
  files_seen: number;
  books_found: number;
  books_added: number;
  formats_added_to_existing: number;
  skipped_already_imported: number;
  skipped_duplicate_content: number;
  skipped_duplicate_title: number;
  added: { book_id: number; title: string; authors: string[]; formats: string[]; source: string }[];
  duplicates: { path: string; duplicate_of: number | null }[];
  skipped_same_format: string[];
  failed: ImportProblem[];
  walk_errors: ImportProblem[];
  unmatched_without_author: number;
  cancelled: boolean;
  /** False when the folder was not entirely seen: errors, or cancelled. */
  conclusive: boolean;
}

const n = (v: number) => v.toLocaleString("en-US");
const plural = (count: number, word: string) => `${n(count)} ${word}${count === 1 ? "" : "s"}`;

/** The outcome as short sentences, most important first. */
export function describeImport(r: ImportReport): string[] {
  const lines: string[] = [];
  lines.push(r.books_added === 0 && r.formats_added_to_existing === 0 ? "Nothing new was added." : `Added ${plural(r.books_added, "book")}${r.formats_added_to_existing ? ` and ${plural(r.formats_added_to_existing, "format")} to existing books` : ""}.`);
  if (r.cancelled) lines.push("Stopped early. Import the folder again to carry on where it left off: what is already in is skipped without being read.");
  if (r.skipped_already_imported) lines.push(`${plural(r.skipped_already_imported, "file")} had been imported before.`);
  if (r.skipped_duplicate_content) lines.push(`${plural(r.skipped_duplicate_content, "file")} skipped: identical to a file already in the library.`);
  if (r.skipped_duplicate_title) lines.push(`${plural(r.skipped_duplicate_title, "file")} skipped: the same title and author as a book already in the library.`);
  if (r.skipped_same_format.length) lines.push(`${plural(r.skipped_same_format.length, "file")} skipped: a second file of the same format in a one-book folder.`);
  if (r.unmatched_without_author) lines.push(`${plural(r.unmatched_without_author, "book")} had no author, so could not be matched by title.`);
  if (r.failed.length) lines.push(`${plural(r.failed.length, "file")} could not be imported.`);
  if (r.walk_errors.length) lines.push(`${plural(r.walk_errors.length, "folder")} could not be read, so the import is incomplete.`);
  return lines;
}

/** Every problem the user may need to act on, as `path: why`. */
export function importProblems(r: ImportReport): string[] {
  return [...r.walk_errors, ...r.failed].map((p) => `${p.path}: ${p.error}`);
}
