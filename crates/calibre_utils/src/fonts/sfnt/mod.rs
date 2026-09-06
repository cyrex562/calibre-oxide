//! Port of `calibre.utils.fonts.sfnt` (the package's own `__init__.py`)
//! plus, so far, `errors.py` and `container.py` (issue #549, split from
//! #64). The fuller per-table object model (`head`/`maxp`/`loca`/
//! `glyf`/`cmap`/`kern`/`metrics`/`gsub`/`merge`/`cff`) is separate,
//! dependency-ordered follow-up scope -- issues #550-555.

pub mod cmap;
pub mod container;
pub mod errors;
pub mod glyf;
pub mod head;
pub mod kern;
pub mod loca;
pub mod maxp;
pub mod metrics;
pub mod subset;

/// Port of `align_block`: pads `raw` with zero bytes until its length
/// is a multiple of 4.
pub fn align_block(raw: &[u8]) -> Vec<u8> {
    let mut out = raw.to_vec();
    let extra = out.len() % 4;
    if extra != 0 {
        out.resize(out.len() + (4 - extra), 0);
    }
    out
}

/// Port of `max_power_of_two`: the highest exponent `e` such that
/// `2**e <= x`.
pub fn max_power_of_two(x: u32) -> u32 {
    let mut x = x;
    let mut exponent = 0u32;
    while x > 0 {
        x >>= 1;
        exponent += 1;
    }
    exponent.saturating_sub(1)
}

/// Port of `load_font`: parses `raw` as a complete sfnt file. The
/// `hasattr(raw, 'read')` stream-reading convenience Python's version
/// has isn't needed -- callers already have `&[u8]` in this port.
pub fn load_font(raw: &[u8]) -> Result<container::Sfnt, errors::UnsupportedFont> {
    container::Sfnt::parse(raw)
}
