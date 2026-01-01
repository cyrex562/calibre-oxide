use std::path::Path;
use unrar::Archive;

pub fn names(path: &Path) -> Result<Vec<String>, String> {
    let filename = path.to_string_lossy().into_owned();
    let mut archive = Some(Archive::new(&filename).open_for_processing().map_err(|e| e.to_string())?);
    let mut names = Vec::new();
    while let Some(a) = archive.take() {
        match a.read_header().map_err(|e| e.to_string())? {
            Some(header) => {
                names.push(header.entry().filename.to_string_lossy().into_owned());
                archive = Some(header.skip().map_err(|e| e.to_string())?);
            }
            None => break,
        }
    }
    Ok(names)
}

pub fn extract_member(path: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
     let filename = path.to_string_lossy().into_owned();
     let mut archive = Some(Archive::new(&filename).open_for_processing().map_err(|e| e.to_string())?);
      while let Some(a) = archive.take() {
         match a.read_header().map_err(|e| e.to_string())? {
            Some(header) => {
                 if header.entry().filename.to_string_lossy() == name {
                     let (data, _) = header.read().map_err(|e| e.to_string())?;
                     return Ok(Some(data));
                 }
                 archive = Some(header.skip().map_err(|e| e.to_string())?);
            }
            None => break,
         }
    }
    Ok(None)
}

pub fn extract(path: &Path, dest: &Path) -> Result<(), String> {
    let filename = path.to_string_lossy().into_owned();
    let mut archive = Some(Archive::new(&filename).open_for_processing().map_err(|e| e.to_string())?);
     while let Some(a) = archive.take() {
        match a.read_header().map_err(|e| e.to_string())? {
            Some(header) => {
                archive = Some(header.extract_to(dest.to_string_lossy().into_owned()).map_err(|e| e.to_string())?);
            }
            None => break,
        }
    }
    Ok(())
}
