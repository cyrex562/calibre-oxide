// The formats the Convert panel offers to convert TO (C15).
//
// The server lists every format the converter can write. A person converting a PDF does not
// want PDF in that list (it is what they have), nor a format nobody reads any more (LIT, LRF), nor two
// names for one format (MARKDOWN and MD, TEXT and TXT).

const OBSOLETE = new Set(["lit", "lrf", "pml", "rb", "tcr"]);

/** The alias that is dropped, and the name it is an alias of. */
const ALIASES: Record<string, string> = { md: "markdown", text: "txt" };

/** `outputs` without the source format, the obsolete formats and the duplicate names. */
export function convertTargets(outputs: readonly string[], source: string): string[] {
  const lower = new Set(outputs.map((f) => f.toLowerCase()));
  const from = source.toLowerCase();
  return outputs.filter((f) => {
    const name = f.toLowerCase();
    if (name === from || OBSOLETE.has(name)) return false;
    const canonical = ALIASES[name];
    return !(canonical && lower.has(canonical));
  });
}

/** The format to preselect: the first target the book does not already have, else the first target. */
export function defaultTarget(targets: readonly string[], have: readonly string[]): string {
  const owned = new Set(have.map((f) => f.toLowerCase()));
  return targets.find((f) => !owned.has(f.toLowerCase())) ?? targets[0] ?? "";
}
