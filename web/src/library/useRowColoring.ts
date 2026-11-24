// Row coloring rules (#4.1).
//
// A rule is a template evaluated per book; a usable color in the result
// colors that row. Evaluation goes through the bulk route -- one request for
// the whole page rather than one per row.
//
// What a template returns is checked against an allowlist before it reaches a
// `style` attribute (./coloringRules.ts): a template is user-authored text and
// can return anything.

import { ref, shallowReactive, watch } from "vue";

import { fetchProfile } from "../settings/api";
import { evaluateTemplateBulk } from "./api";
import type { BookWindow } from "./bookWindow";
import { activeRules, colorForBook, COLORING_RULES_PROFILE, DEFAULT_COLORING_RULES, type ColoringRule, type ColoringRulesPrefs } from "./coloringRules";

export function useRowColoring(win: BookWindow) {
  const coloringRules = ref<ColoringRule[]>([]);
  // A reactive Map, changed key by key: a chunk of rows arriving used to copy
  // the whole record (every colored row of a 50,000-book list) to add a few.
  const rowColors = shallowReactive(new Map<number, string>());

  async function loadColoringRules() {
    try {
      const prefs = await fetchProfile<ColoringRulesPrefs>(COLORING_RULES_PROFILE);
      coloringRules.value = prefs?.rules ?? DEFAULT_COLORING_RULES.rules;
    } catch (e) {
      console.error("failed to load coloring rules", e);
    }
  }
  void loadColoringRules();

  /** Rows currently rendered (visible plus overscan), as reported by the table/grid. */
  const renderedRange = ref({ first: 0, last: 0 });
  /** Ids already evaluated against the current rules, so scrolling does not re-ask. */
  const colorChecked = new Set<number>();
  let colorGeneration = 0;

  async function applyColoringRules() {
    const rules = activeRules(coloringRules.value);
    if (rules.length === 0) {
      rowColors.clear();
      colorChecked.clear();
      return;
    }
    // Only the rows on screen that have loaded and have not been colored yet.
    const { first, last } = renderedRange.value;
    const ids = win.ids.value.slice(first, last).filter((id) => win.cache.has(id) && !colorChecked.has(id));
    if (ids.length === 0) return;
    const generation = colorGeneration;
    for (const id of ids) colorChecked.add(id);
    try {
      // One request per rule, not per book: a rule is the same template
      // for every row, and the server parses it once.
      const perRule = await Promise.all(rules.map((r) => evaluateTemplateBulk(r.template, ids)));
      if (generation !== colorGeneration) return;
      for (const id of ids) {
        const color = colorForBook(rules, perRule.map((results) => results[String(id)]));
        if (color) rowColors.set(id, color);
        else rowColors.delete(id);
      }
    } catch (e) {
      // Coloring is decoration; a failure must not take down the list.
      console.error("failed to apply coloring rules", e);
      for (const id of ids) colorChecked.delete(id);
    }
  }

  // Re-run whenever rows load, the visible set moves, or the rules change.
  watch([win.version, renderedRange], () => void applyColoringRules());
  // A new query: colors belong to the old rows.
  watch(win.resetCount, () => {
    colorGeneration++;
    colorChecked.clear();
    rowColors.clear();
  });
  watch(coloringRules, () => {
    colorGeneration++;
    colorChecked.clear();
    rowColors.clear();
    void applyColoringRules();
  }, { deep: true });

  return {
    rowColors,
    /** Rows currently rendered, as reported by the table/grid. */
    renderedRange,
    /** The rows' colors are stale (the list reloaded): ask again for the ones on screen. */
    forgetAll: () => colorChecked.clear(),
    /** One row's values changed: ask again for it. */
    forget: (id: number) => colorChecked.delete(id),
    apply: applyColoringRules,
  };
}
