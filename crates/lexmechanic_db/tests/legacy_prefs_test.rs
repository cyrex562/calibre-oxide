//! Preferences written before the rename live under `oxide.*`. They are read
//! through the new `lexmechanic.*` name, and a write moves them over.

use lexmechanic_db::cache::Cache;
use lexmechanic_db::{library_folders, naming};

fn new_cache() -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    (dir, cache)
}

#[test]
fn legacy_key_is_derived_for_our_keys_only() {
    assert_eq!(Cache::legacy_preference_key("lexmechanic.naming").as_deref(), Some("oxide.naming"));
    assert_eq!(Cache::legacy_preference_key("fts.enabled"), None);
}

#[test]
fn the_old_key_is_read_when_the_new_one_is_missing() {
    let (_dir, cache) = new_cache();
    cache.set_preference("oxide.naming", "\"old\"").unwrap();
    assert_eq!(cache.get_preference_compat("lexmechanic.naming").unwrap().as_deref(), Some("\"old\""));
}

#[test]
fn the_new_key_wins() {
    let (_dir, cache) = new_cache();
    cache.set_preference("oxide.naming", "\"old\"").unwrap();
    cache.set_preference("lexmechanic.naming", "\"new\"").unwrap();
    assert_eq!(cache.get_preference_compat("lexmechanic.naming").unwrap().as_deref(), Some("\"new\""));
}

#[test]
fn a_write_goes_to_the_new_key_and_retires_the_old_one() {
    let (_dir, cache) = new_cache();
    cache.set_preference("oxide.naming", "\"old\"").unwrap();
    cache.set_preference_compat("lexmechanic.naming", "\"new\"").unwrap();
    assert_eq!(cache.get_preference("lexmechanic.naming").unwrap().as_deref(), Some("\"new\""));
    assert_eq!(cache.get_preference("oxide.naming").unwrap(), None);
}

#[test]
fn clearing_clears_both_so_the_old_value_cannot_come_back() {
    let (_dir, cache) = new_cache();
    cache.set_preference("oxide.naming", "\"old\"").unwrap();
    cache.set_preference("lexmechanic.naming", "\"new\"").unwrap();
    cache.clear_preference_compat("lexmechanic.naming").unwrap();
    assert_eq!(cache.get_preference_compat("lexmechanic.naming").unwrap(), None);
    assert_eq!(cache.get_preference("oxide.naming").unwrap(), None);
}

#[test]
fn the_real_users_of_the_keys_fall_back() {
    let (_dir, cache) = new_cache();
    // A library whose empty-folders list was stored before the rename.
    cache.set_preference(&Cache::legacy_preference_key(library_folders::EMPTY_FOLDERS_PREF).unwrap(), "[\"Receipts\"]").unwrap();
    assert!(library_folders::EMPTY_FOLDERS_PREF.starts_with("lexmechanic."));
    assert_eq!(cache.get_preference_compat(library_folders::EMPTY_FOLDERS_PREF).unwrap().as_deref(), Some("[\"Receipts\"]"));
    assert!(naming::PREF_KEY.starts_with("lexmechanic."));
}
