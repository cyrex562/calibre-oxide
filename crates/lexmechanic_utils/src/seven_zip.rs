use std::path::Path;
use std::fs::File;

pub fn open_archive(path: &Path) -> Result<sevenz_rust::SevenZReader<File>, String> {
    sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty()).map_err(|e| e.to_string())
}

pub fn names(path: &Path) -> Result<Vec<String>, String> {
    let mut archive = open_archive(path)?;
    let mut names = Vec::new();
    archive.for_each_entries(|entry, _| {
        names.push(entry.name().to_string());
        Ok(true)
    }).map_err(|e| e.to_string())?;
    Ok(names)
}

pub fn extract_member(path: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
     let mut archive = open_archive(path)?;
     archive.for_each_entries(|entry, _| {
         if entry.name() == name {
             // Still placeholder logic as we verify API
             Ok(true) 
         } else {
             Ok(true)
         }
     }).map_err(|e| e.to_string())?;
     
     // Placeholder
     let mut archive = open_archive(path)?;
     if let Some(_entry) = archive.archive().files.iter().find(|e| e.name == name).cloned() {
         return Ok(None);
     }
     
     Ok(None)
}

pub fn extract(path: &Path, dest: &Path) -> Result<(), String> {
    sevenz_rust::decompress_file(path, dest).map_err(|e| e.to_string())
}
