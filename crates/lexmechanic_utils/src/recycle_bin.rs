use std::path::Path;
use std::fs;

pub fn recycle(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}

pub fn delete_file(path: &Path, permanent: bool) -> Result<(), String> {
    if !permanent {
        if let Ok(_) = recycle(path) {
            return Ok(());
        }
    }
    // Fallback or permanent
    fs::remove_file(path).map_err(|e| e.to_string())
}

pub fn delete_tree(path: &Path, permanent: bool) -> Result<(), String> {
    if !permanent {
       if let Ok(_) = recycle(path) {
           return Ok(());
       }
    }
    fs::remove_dir_all(path).map_err(|e| e.to_string())
}
