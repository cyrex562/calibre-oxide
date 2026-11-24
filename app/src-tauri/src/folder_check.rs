//! What to say before a picked folder becomes the open library.
//!
//! "Open a library" used to hand any folder straight to the server, which
//! creates `metadata.db` in an empty or unrelated folder without a word.
//! Pointing it at `D:\Documents` by mistake then scatters a database and
//! sidecars among someone's files. Two things are worth a question first:
//! the folder is not a library yet, and the folder is on a network drive.

use std::path::Path;

use lexmechanic_db::library_handle::{classify_storage_tier, StorageTier};

/// Asked when the picked folder has no `metadata.db`.
pub const NEW_LIBRARY_PROMPT: &str = "This folder has no library yet. Create a new library here? Your files stay where they are; you can then scan the folder to add them.";

/// Whether `folder` already holds a library.
pub fn is_existing_library(folder: &Path) -> bool {
    folder.join("metadata.db").is_file()
}

/// Whether `folder` is on a network drive (or a share mounted as one).
pub fn is_on_network_drive(folder: &Path) -> bool {
    classify_storage_tier(folder) == StorageTier::Network
}

/// The warning shown for a library on a network drive.
pub fn network_warning(folder: &Path) -> String {
    format!(
        "{} is on a network drive.\n\nA library works best on a local disk: a slow or interrupted connection can make it sluggish or leave it in a bad state. Working on a local copy is recommended.",
        folder.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_folder_with_metadata_db_is_a_library() {
        let d = tempfile::tempdir().unwrap();
        assert!(!is_existing_library(d.path()));
        std::fs::create_dir(d.path().join("metadata.db")).unwrap();
        assert!(!is_existing_library(d.path()), "a directory named metadata.db is not a database");
        let d2 = tempfile::tempdir().unwrap();
        std::fs::write(d2.path().join("metadata.db"), b"").unwrap();
        assert!(is_existing_library(d2.path()));
    }

    #[test]
    fn a_temp_folder_is_not_a_network_drive() {
        let d = tempfile::tempdir().unwrap();
        assert!(!is_on_network_drive(d.path()));
    }

    #[test]
    fn the_warning_names_the_folder_and_recommends_a_local_copy() {
        let w = network_warning(Path::new("//nas/books"));
        assert!(w.contains("nas") && w.contains("local copy"));
    }

    #[test]
    fn the_prompt_says_files_stay_and_scanning_follows() {
        assert!(NEW_LIBRARY_PROMPT.starts_with("This folder has no library yet. Create a new library here?"));
        assert!(NEW_LIBRARY_PROMPT.contains("Your files stay where they are"));
    }
}
