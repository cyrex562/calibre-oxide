//! A library last opened before the project was renamed keeps its state in
//! `.calibre-oxide/`. Opening it moves that to `.lexmechanic/`.

use lexmechanic_db::backend::Backend;
use lexmechanic_db::constants::{LEGACY_LIBRARY_HANDLE_DIR_NAME, LIBRARY_HANDLE_DIR_NAME};
use lexmechanic_db::library_handle::{migrate_legacy_state_dir, LibraryHandle, StateDirMigration};
use std::fs;

#[test]
fn the_legacy_directory_is_renamed_with_its_contents() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join(LEGACY_LIBRARY_HANDLE_DIR_NAME);
    fs::create_dir_all(legacy.join("changes")).unwrap();
    fs::write(legacy.join("changes").join("0001.json"), b"{}").unwrap();
    fs::write(legacy.join("install-id"), b"abc").unwrap();

    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::Migrated);

    assert!(!legacy.exists());
    let current = dir.path().join(LIBRARY_HANDLE_DIR_NAME);
    assert_eq!(fs::read(current.join("changes").join("0001.json")).unwrap(), b"{}");
    assert_eq!(fs::read_to_string(current.join("install-id")).unwrap(), "abc");
}

#[test]
fn nothing_to_do_without_a_legacy_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::NotNeeded);
    fs::create_dir_all(dir.path().join(LIBRARY_HANDLE_DIR_NAME)).unwrap();
    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::NotNeeded);
}

#[test]
fn when_both_exist_the_new_one_is_used_and_the_old_one_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join(LEGACY_LIBRARY_HANDLE_DIR_NAME);
    let current = dir.path().join(LIBRARY_HANDLE_DIR_NAME);
    fs::create_dir_all(&legacy).unwrap();
    fs::create_dir_all(&current).unwrap();
    fs::write(legacy.join("old.txt"), b"old").unwrap();
    fs::write(current.join("new.txt"), b"new").unwrap();

    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::BothPresent);

    assert_eq!(fs::read(legacy.join("old.txt")).unwrap(), b"old");
    assert_eq!(fs::read(current.join("new.txt")).unwrap(), b"new");
    assert!(!current.join("old.txt").exists(), "nothing is merged");
}

#[test]
fn it_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(LEGACY_LIBRARY_HANDLE_DIR_NAME)).unwrap();
    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::Migrated);
    assert_eq!(migrate_legacy_state_dir(dir.path()).unwrap(), StateDirMigration::NotNeeded);
}

#[test]
fn opening_a_backend_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join(LEGACY_LIBRARY_HANDLE_DIR_NAME);
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("marker"), b"kept").unwrap();

    let _backend = Backend::new(dir.path()).unwrap();

    assert!(!legacy.exists());
    assert_eq!(fs::read(dir.path().join(LIBRARY_HANDLE_DIR_NAME).join("marker")).unwrap(), b"kept");
}

#[test]
fn opening_a_write_handle_migrates_and_keeps_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join(LEGACY_LIBRARY_HANDLE_DIR_NAME);
    fs::create_dir_all(legacy.join("journal")).unwrap();
    fs::write(legacy.join("marker"), b"kept").unwrap();

    let _handle = LibraryHandle::open(dir.path()).unwrap();

    assert!(!legacy.exists());
    let current = dir.path().join(LIBRARY_HANDLE_DIR_NAME);
    assert_eq!(fs::read(current.join("marker")).unwrap(), b"kept");
    assert!(current.join("journal").is_dir());
    assert!(current.join("writer.lock").is_file());
}
