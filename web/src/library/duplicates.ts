// What the Find duplicates panel says about a group: how strong the evidence
// is, how to filter by it, and what it is called.
//
// The server (crates/calibre_srv/src/duplicates.rs) reports every group it
// finds with a `reason`, a `confidence` and the `evidence` that joined it;
// the rules for what counts as evidence are in crates/calibre_db/src/dedupe.rs.
// Everything the panel needs to *decide* lives here, away from the template,
// so it can be tested.

import type { DuplicateGroup, DuplicateReason } from "./api";

/** The weakest evidence the panel shows. */
export type DuplicateMin = "fuzzy" | "exact" | "identifier" | "content";

/** Strongest first. A higher rank is stronger evidence. */
const RANK: Record<DuplicateReason, number> = {
  content: 4,
  identifier: 3,
  title_author_exact: 2,
  title_author_fuzzy: 1,
};

const MIN_RANK: Record<DuplicateMin, number> = { content: 4, identifier: 3, exact: 2, fuzzy: 1 };

export const MIN_OPTIONS: { value: DuplicateMin; label: string }[] = [
  { value: "fuzzy", label: "Everything, including probable matches" },
  { value: "exact", label: "Same title and author, and stronger" },
  { value: "identifier", label: "Same ISBN or identifier, and stronger" },
  { value: "content", label: "Identical files only" },
];

const LABELS: Record<DuplicateReason, string> = {
  content: "identical files",
  identifier: "same ISBN or identifier",
  title_author_exact: "same title and author",
  title_author_fuzzy: "probably the same book",
};

export function reasonLabel(reason: DuplicateReason): string {
  return LABELS[reason] ?? reason;
}

export type BadgeTone = "certain" | "strong" | "likely" | "review";

/** The short badge for a reason. `review` means a person must confirm. */
export function reasonBadge(reason: DuplicateReason): { text: string; tone: BadgeTone } {
  switch (reason) {
    case "content":
      return { text: "Identical file", tone: "certain" };
    case "identifier":
      return { text: "Same ISBN / ID", tone: "strong" };
    case "title_author_exact":
      return { text: "Same title + author", tone: "likely" };
    default:
      return { text: "Possible match", tone: "review" };
  }
}

/** 0.9 -> "90%". */
export function confidencePercent(confidence: number): string {
  return `${Math.round(confidence * 100)}%`;
}

/**
 * Whether the group is a guess. Only an identical file or a shared identifier
 * is decided by rule; a title match, however exact, can be two books that
 * share a title and an author's name.
 */
export function needsReview(group: Pick<DuplicateGroup, "reason">): boolean {
  return group.reason === "title_author_fuzzy";
}

/** Groups at least as strong as `min`. Order is kept. */
export function filterGroups<G extends Pick<DuplicateGroup, "reason">>(groups: G[], min: DuplicateMin): G[] {
  const floor = MIN_RANK[min] ?? 1;
  return groups.filter((g) => (RANK[g.reason] ?? 0) >= floor);
}

/**
 * How many groups each filter would show, keyed by the filter's value, so the
 * dropdown can say what choosing it does.
 */
export function countByReason(groups: Pick<DuplicateGroup, "reason">[]): Record<DuplicateMin, number> {
  const out: Record<DuplicateMin, number> = { fuzzy: 0, exact: 0, identifier: 0, content: 0 };
  for (const min of Object.keys(out) as DuplicateMin[]) {
    out[min] = filterGroups(groups, min).length;
  }
  return out;
}

/** One line per piece of evidence, for a tooltip. */
export function evidenceLines(group: Pick<DuplicateGroup, "evidence">): string[] {
  return (group.evidence ?? []).slice(0, 8).map((e) => `${reasonLabel(e.reason)} (${confidencePercent(e.confidence)}): ${e.detail}`);
}
