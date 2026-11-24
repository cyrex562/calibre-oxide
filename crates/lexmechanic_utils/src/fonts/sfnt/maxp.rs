//! Port of `calibre.utils.fonts.sfnt.maxp` (`MaxpTable`, issue #550).

use super::errors::UnsupportedFont;
use crate::fonts::utils::Cursor;

/// The version-1.0-only fields, present only when [`MaxpTable::version`]
/// is `0x0001_0000` (`1.0` in 16.16 fixed-point).
#[derive(Debug, Clone, Default)]
pub struct MaxpV1Fields {
    pub max_points: u16,
    pub max_contours: u16,
    pub max_composite_points: u16,
    pub max_composite_contours: u16,
    pub max_zones: u16,
    pub max_twilight_points: u16,
    pub max_storage: u16,
    pub max_function_defs: u16,
    pub max_instruction_defs: u16,
    pub max_stack_elements: u16,
    pub max_size_of_instructions: u16,
    pub max_component_elements: u16,
    pub max_component_depth: u16,
}

/// Port of `MaxpTable`. `version` is the raw 16.16 fixed-point `i32`
/// (see [`super::head::HeadTable`]'s own doc for why `FixedProperty`
/// isn't ported as a separate abstraction).
#[derive(Debug, Clone)]
pub struct MaxpTable {
    pub version: i32,
    pub num_glyphs: u16,
    pub v1: Option<MaxpV1Fields>,
}

impl MaxpTable {
    /// Port of `MaxpTable.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self, UnsupportedFont> {
        let mut c = Cursor::new(raw);
        let version = c.i32().map_err(UnsupportedFont)?;
        let num_glyphs = c.u16().map_err(UnsupportedFont)?;
        let version_f = version as f64 / 65536.0;
        if version_f > 1.0 {
            return Err(UnsupportedFont(format!("This font has a maxp table with version: {version_f}")));
        }
        let v1 = if version_f == 1.0 {
            let get = |r: Result<u16, String>| r.map_err(UnsupportedFont);
            Some(MaxpV1Fields {
                max_points: get(c.u16())?,
                max_contours: get(c.u16())?,
                max_composite_points: get(c.u16())?,
                max_composite_contours: get(c.u16())?,
                max_zones: get(c.u16())?,
                max_twilight_points: get(c.u16())?,
                max_storage: get(c.u16())?,
                max_function_defs: get(c.u16())?,
                max_instruction_defs: get(c.u16())?,
                max_stack_elements: get(c.u16())?,
                max_size_of_instructions: get(c.u16())?,
                max_component_elements: get(c.u16())?,
                max_component_depth: get(c.u16())?,
            })
        } else {
            None
        };
        Ok(MaxpTable { version, num_glyphs, v1 })
    }

    /// Port of `MaxpTable.update`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.num_glyphs.to_be_bytes());
        if let Some(v1) = &self.v1 {
            for v in [
                v1.max_points,
                v1.max_contours,
                v1.max_composite_points,
                v1.max_composite_contours,
                v1.max_zones,
                v1.max_twilight_points,
                v1.max_storage,
                v1.max_function_defs,
                v1.max_instruction_defs,
                v1.max_stack_elements,
                v1.max_size_of_instructions,
                v1.max_component_elements,
                v1.max_component_depth,
            ] {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_0_5_table_has_only_num_glyphs() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(0x0000_5000i32).to_be_bytes()); // 0.3125, < 1.0
        raw.extend_from_slice(&42u16.to_be_bytes());
        let table = MaxpTable::parse(&raw).unwrap();
        assert_eq!(table.num_glyphs, 42);
        assert!(table.v1.is_none());
        assert_eq!(table.to_bytes(), raw);
    }

    #[test]
    fn version_1_0_table_has_the_full_field_set() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(0x0001_0000i32).to_be_bytes());
        raw.extend_from_slice(&10u16.to_be_bytes()); // num_glyphs
        for v in 1u16..=13 {
            raw.extend_from_slice(&v.to_be_bytes());
        }
        let table = MaxpTable::parse(&raw).unwrap();
        let v1 = table.v1.as_ref().unwrap();
        assert_eq!(v1.max_points, 1);
        assert_eq!(v1.max_component_depth, 13);
        assert_eq!(table.to_bytes(), raw);
    }

    #[test]
    fn rejects_a_version_greater_than_one() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(0x0002_0000i32).to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        let err = MaxpTable::parse(&raw).unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
    }
}
