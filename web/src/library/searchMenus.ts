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

export type VlMenuId = typeof VL_NONE | typeof VL_MANAGE | `vl:${string}`;
export type SavedMenuId = typeof SAVED_MANAGE | `ss:${string}`;

export interface MenuItem<T extends string> {
  id: T;
  label: string;
  enabled: boolean;
  checked?: boolean;
  startsGroup?: boolean;
}

export function vlMenuItems(names: string[], active: string): MenuItem<VlMenuId>[] {
  const items: MenuItem<VlMenuId>[] = [
    { id: VL_NONE, label: "All books", enabled: true, checked: active === "" },
  ];
  for (const name of names) {
    items.push({ id: `vl:${name}`, label: name, enabled: true, checked: active === name });
  }
  items.push({ id: VL_MANAGE, label: "Manage virtual libraries…", enabled: true, startsGroup: true });
  return items;
}

export type VlChoice = { kind: "apply"; name: string } | { kind: "manage" };

export function readVlChoice(id: VlMenuId): VlChoice {
  if (id === VL_MANAGE) return { kind: "manage" };
  if (id === VL_NONE) return { kind: "apply", name: "" };
  return { kind: "apply", name: id.slice("vl:".length) };
}

export function savedMenuItems(names: string[]): MenuItem<SavedMenuId>[] {
  const items: MenuItem<SavedMenuId>[] = names.map((name) => ({
    id: `ss:${name}` as SavedMenuId,
    label: name,
    enabled: true,
  }));
  // Always offered, so an empty list still explains where saved
  // searches come from instead of showing a bare "No actions".
  items.push({ id: SAVED_MANAGE, label: "Manage saved searches…", enabled: true, startsGroup: items.length > 0 });
  return items;
}

export type SavedChoice = { kind: "apply"; name: string } | { kind: "manage" };

export function readSavedChoice(id: SavedMenuId): SavedChoice {
  if (id === SAVED_MANAGE) return { kind: "manage" };
  return { kind: "apply", name: id.slice("ss:".length) };
}
