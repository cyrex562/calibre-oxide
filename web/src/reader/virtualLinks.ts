// Port of the client side of render_book.py's `encode_url`/`decode_url`
// virtualization scheme, real signatures matching
// calibre_ebooks::link_virtualize::{encode_url,decode_url} exactly.
//
// A virtualized reference looks like `link_uid|base64(name)[#frag]|`
// (see that module's own doc for why). Used for non-anchor attributes
// (img/link src/href, inline `style=`/`<style>` text `url(...)`
// references) -- `<a>`/`<area>` elements instead carry their target in
// a `data-{link_uid}` JSON attribute (see anchorLinkData below),
// matching `process_anchor_links`'s real output shape.

export interface DecodedLink {
  name: string;
  frag: string;
}

export function decodeVirtualComponent(encoded: string): DecodedLink | null {
  const hashIdx = encoded.indexOf("#");
  const namePart = hashIdx === -1 ? encoded : encoded.slice(0, hashIdx);
  const frag = hashIdx === -1 ? "" : encoded.slice(hashIdx + 1);
  try {
    const name = atob(namePart);
    return { name, frag };
  } catch {
    return null;
  }
}

/** Replaces every `link_uid|...|` occurrence in `text` via `resolve`. */
export function rewriteVirtualLinks(text: string, linkUid: string, resolve: (link: DecodedLink) => string | null): string {
  const pattern = new RegExp(`${escapeRegExp(linkUid)}\\|([^|]+)\\|`, "g");
  return text.replace(pattern, (whole, encoded: string) => {
    const decoded = decodeVirtualComponent(encoded);
    if (!decoded) return whole;
    const replacement = resolve(decoded);
    return replacement ?? whole;
  });
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export interface AnchorLinkData {
  name: string;
  frag: string;
  missing?: boolean;
}

/** Port of reading `data-{link_uid}` off an `<a>`/`<area>` element,
 * matching `process_anchor_links`'s real output shape exactly. */
export function anchorLinkData(el: Element, linkUid: string): AnchorLinkData | null {
  const raw = el.getAttribute(`data-${linkUid}`);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as AnchorLinkData;
  } catch {
    return null;
  }
}
