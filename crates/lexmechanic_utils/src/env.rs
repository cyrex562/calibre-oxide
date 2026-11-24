//! Environment variables, with a fallback to the names the project had
//! before it was renamed.
//!
//! Every setting is now `LEXMECHANIC_*`. People who set the old
//! `CALIBRE_OXIDE_*` name in a shell profile, a service definition or a
//! launcher keep working: [`var`]/[`var_os`] read the new name first and fall
//! back to the old one. The new name always wins when both are set.

use std::ffi::{OsStr, OsString};

/// Prefix of every setting's current name.
pub const PREFIX: &str = "LEXMECHANIC_";
/// Prefix the same settings had before the rename.
pub const LEGACY_PREFIX: &str = "CALIBRE_OXIDE_";

/// The pre-rename spelling of a `LEXMECHANIC_*` name, or `None` for a name
/// that does not carry the prefix.
pub fn legacy_name(name: &str) -> Option<String> {
    name.strip_prefix(PREFIX).map(|rest| format!("{LEGACY_PREFIX}{rest}"))
}

/// [`var_os`] over an arbitrary lookup, so the fallback can be tested without
/// touching the process environment.
pub fn var_os_with(lookup: impl Fn(&str) -> Option<OsString>, name: &str) -> Option<OsString> {
    if let Some(value) = lookup(name) {
        return Some(value);
    }
    legacy_name(name).and_then(|old| lookup(&old))
}

/// The value of `LEXMECHANIC_*` setting `name`, else of its pre-rename
/// `CALIBRE_OXIDE_*` spelling.
pub fn var_os(name: impl AsRef<OsStr>) -> Option<OsString> {
    let name = name.as_ref().to_string_lossy().into_owned();
    var_os_with(|n| std::env::var_os(n), &name)
}

/// [`var_os`], as a UTF-8 string (a non-UTF-8 value reads as unset).
pub fn var(name: &str) -> Result<String, std::env::VarError> {
    match var_os(name) {
        Some(v) => v.into_string().map_err(std::env::VarError::NotUnicode),
        None => Err(std::env::VarError::NotPresent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn legacy_name_swaps_the_prefix() {
        assert_eq!(legacy_name("LEXMECHANIC_PDFIUM").as_deref(), Some("CALIBRE_OXIDE_PDFIUM"));
        assert_eq!(legacy_name("PATH"), None);
    }

    #[test]
    fn the_new_name_is_read_first() {
        let l = lookup(&[("LEXMECHANIC_X", "new")]);
        assert_eq!(var_os_with(l, "LEXMECHANIC_X"), Some(OsString::from("new")));
    }

    #[test]
    fn the_old_name_is_the_fallback() {
        let l = lookup(&[("CALIBRE_OXIDE_X", "old")]);
        assert_eq!(var_os_with(l, "LEXMECHANIC_X"), Some(OsString::from("old")));
    }

    #[test]
    fn the_new_name_wins_when_both_are_set() {
        let l = lookup(&[("CALIBRE_OXIDE_X", "old"), ("LEXMECHANIC_X", "new")]);
        assert_eq!(var_os_with(l, "LEXMECHANIC_X"), Some(OsString::from("new")));
    }

    #[test]
    fn unset_is_unset_and_other_names_do_not_fall_back() {
        assert_eq!(var_os_with(lookup(&[]), "LEXMECHANIC_X"), None);
        assert_eq!(var_os_with(lookup(&[("CALIBRE_OXIDE_X", "old")]), "SOMETHING_ELSE"), None);
    }

    #[test]
    fn real_environment_falls_back() {
        // Unique names so parallel tests never see each other's values.
        std::env::set_var("CALIBRE_OXIDE_ENVCOMPAT_TEST_ONLY", "legacy");
        assert_eq!(var("LEXMECHANIC_ENVCOMPAT_TEST_ONLY").unwrap(), "legacy");
        std::env::set_var("LEXMECHANIC_ENVCOMPAT_TEST_ONLY", "current");
        assert_eq!(var("LEXMECHANIC_ENVCOMPAT_TEST_ONLY").unwrap(), "current");
        std::env::remove_var("LEXMECHANIC_ENVCOMPAT_TEST_ONLY");
        std::env::remove_var("CALIBRE_OXIDE_ENVCOMPAT_TEST_ONLY");
        assert!(var("LEXMECHANIC_ENVCOMPAT_TEST_ONLY").is_err());
    }
}
