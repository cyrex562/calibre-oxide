// The virtual-library and saved-search dropdowns.
//
// Both replaced a `<select>`. A select cannot manage its own options,
// so creating or deleting a virtual library lived in a separate dialog
// with no relationship to the control that applies one; and the
// saved-search select used a `disabled selected` placeholder as its
// label, which is the standard workaround for a select pretending to
// be a menu.
//
// The parsing lives here rather than in the component because both
// menus mix real names with sentinel ids, and a name that collides
// with a sentinel -- or a mis-sliced prefix -- silently applies the
// wrong thing instead of failing.

/** Shown when no virtual library is active. */
export const VL_NONE = "vl:__none__";
/** Opens the management dialog rather than applying anything. */
export const VL_MANAGE = "vl:__manage__";
export const SAVED_MANAGE = "ss:__manage__";
/** Saves the search that is on screen as a saved search. */
export const SAVED_SAVE = "ss:__save__";
/** Makes a virtual library from the search that is on screen. */
export const VL_FROM_SEARCH = "vl:__from_search__";

export type VlMenuId = typeof VL_NONE | typeof VL_MANAGE | typeof VL_FROM_SEARCH | `vl:${string}`;
export type SavedMenuId = typeof SAVED_MANAGE | typeof SAVED_SAVE | `ss:${string}`;

export interface MenuItem<T extends string> {
  id: T;
  label: string;
  enabled: boolean;
  checked?: boolean;
  startsGroup?: boolean;
}

/** `query` is the search on screen: with one, the menu offers to turn it into a virtual library. */
export function vlMenuItems(names: string[], active: string, query = ""): MenuItem<VlMenuId>[] {
  const items: MenuItem<VlMenuId>[] = [
    { id: VL_NONE, label: "All books", enabled: true, checked: active === "" },
  ];
  for (const name of names) {
    items.push({ id: `vl:${name}`, label: name, enabled: true, checked: active === name });
  }
  if (query.trim()) items.push({ id: VL_FROM_SEARCH, label: "Make a virtual library from this search…", enabled: true, startsGroup: true });
  items.push({ id: VL_MANAGE, label: "Manage virtual libraries…", enabled: true, startsGroup: !query.trim() });
  return items;
}

export type VlChoice = { kind: "apply"; name: string } | { kind: "manage" } | { kind: "from-search" };

export function readVlChoice(id: VlMenuId): VlChoice {
  if (id === VL_MANAGE) return { kind: "manage" };
  if (id === VL_FROM_SEARCH) return { kind: "from-search" };
  if (id === VL_NONE) return { kind: "apply", name: "" };
  return { kind: "apply", name: id.slice("vl:".length) };
}

/** `query` is the search on screen: with one, "Save this search…" leads the menu. */
export function savedMenuItems(names: string[], query = ""): MenuItem<SavedMenuId>[] {
  const items: MenuItem<SavedMenuId>[] = [];
  if (query.trim()) items.push({ id: SAVED_SAVE, label: "Save this search…", enabled: true });
  items.push(
    ...names.map((name) => ({
      id: `ss:${name}` as SavedMenuId,
      label: name,
      enabled: true,
      ...(query.trim() && names[0] === name ? { startsGroup: true } : {}),
    })),
  );
  // Always offered, so an empty list still explains where saved
  // searches come from instead of showing a bare "No actions".
  items.push({ id: SAVED_MANAGE, label: "Manage saved searches…", enabled: true, startsGroup: items.length > 0 });
  return items;
}

export type SavedChoice = { kind: "apply"; name: string } | { kind: "manage" } | { kind: "save" };

export function readSavedChoice(id: SavedMenuId): SavedChoice {
  if (id === SAVED_MANAGE) return { kind: "manage" };
  if (id === SAVED_SAVE) return { kind: "save" };
  return { kind: "apply", name: id.slice("ss:".length) };
}
