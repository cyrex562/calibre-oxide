// How Check library words one finding (C28).

import type { CheckLibraryFinding } from "./api";

/**
 * What a finding is called in the list: the book's title when it has one, else the file
 * name from its path (a file in the folder that no book claims has no title, and a row that
 * says "—" for it hides the one thing worth reading).
 */
export function findingName(finding: Pick<CheckLibraryFinding, "title" | "path">): string {
  if (finding.title) return finding.title;
  const parts = finding.path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? "—";
}
