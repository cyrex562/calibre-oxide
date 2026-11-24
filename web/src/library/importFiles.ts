// Adding book files one by one (the "Add books…" picker), and what to do about
// the ones the library already seems to have.
//
// The server answers an add of a book with the same title and author by
// refusing it and listing what exists, and takes the file again with the
// "add duplicates" flag. There is no "add the file you already got" route, so
// a duplicate the user then chooses to add is uploaded a second time; the
// ones they skip are never uploaded again. What changed is the asking: every
// duplicate used to stop the import with its own native confirm, one per
// file. Now the import runs to the end and the duplicates come back as one
// list to decide on at once.

import type { AddBookResult } from "./types";

export type DuplicateMode = "ask" | "add" | "skip";

export type AddFn = (file: File, addDuplicates: boolean) => Promise<AddBookResult>;

export interface PendingDuplicate {
  file: File;
  /** The books already in the library that look the same, as "Title (Author & Author)". */
  existing: string[];
}

export interface ImportOutcome {
  added: number;
  skipped: number;
  errors: string[];
  /** Files held back for one decision at the end. Only in "ask" mode. */
  duplicates: PendingDuplicate[];
}

function existingNames(result: AddBookResult): string[] {
  return (result.duplicates ?? []).map((d) => `${d.title} (${d.authors.join(" & ")})`);
}

function isDuplicate(result: AddBookResult): boolean {
  return (result.duplicates?.length ?? 0) > 0 && result.book_id === undefined;
}

/** Uploads each file; duplicates are skipped, added, or (in "ask" mode) collected. */
export async function importFiles(files: File[], add: AddFn, mode: DuplicateMode): Promise<ImportOutcome> {
  const out: ImportOutcome = { added: 0, skipped: 0, errors: [], duplicates: [] };
  for (const file of files) {
    try {
      const result = await add(file, false);
      if (!isDuplicate(result)) {
        out.added += 1;
      } else if (mode === "skip") {
        out.skipped += 1;
      } else if (mode === "add") {
        await add(file, true);
        out.added += 1;
      } else {
        out.duplicates.push({ file, existing: existingNames(result) });
      }
    } catch (e) {
      out.errors.push(`${file.name}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  return out;
}

/** Uploads the duplicates the user chose to add. */
export async function addChosenDuplicates(chosen: PendingDuplicate[], add: AddFn): Promise<{ added: number; errors: string[] }> {
  let added = 0;
  const errors: string[] = [];
  for (const d of chosen) {
    try {
      await add(d.file, true);
      added += 1;
    } catch (e) {
      errors.push(`${d.file.name}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  return { added, errors };
}

/** The line shown after an import. */
export function describeImport(total: number, added: number, skipped: number, failed: number): string {
  return `Added ${added} of ${total} book(s)${skipped ? `, ${skipped} skipped` : ""}${failed ? `, ${failed} failed` : ""}.`;
}
