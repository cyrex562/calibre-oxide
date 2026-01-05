use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub trait Container {
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<()>;
    fn exists(&self, path: &str) -> bool;
    fn namelist(&self) -> Result<Vec<String>>;
}

pub struct NullContainer;

impl NullContainer {
    pub fn new() -> Self {
        NullContainer
    }
}

impl Default for NullContainer {
    fn default() -> Self {
        NullContainer::new()
    }
}

impl Container for NullContainer {
    fn read(&self, _path: &str) -> Result<Vec<u8>> {
        bail!("Attempt to read from NullContainer")
    }
    fn write(&mut self, _path: &str, _data: &[u8]) -> Result<()> {
        bail!("Attempt to write to NullContainer")
    }
    fn exists(&self, _path: &str) -> bool {
        false
    }
    fn namelist(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

pub struct DirContainer {
    root: PathBuf,
}

impl DirContainer {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        DirContainer {
            root: path.as_ref().to_path_buf(),
        }
    }
}

impl Container for DirContainer {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let p = self.root.join(path);
        Ok(fs::read(p)?)
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let p = self.root.join(path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, data)?;
        Ok(())
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }

    fn namelist(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        // naive recursive walk
        let walker = ignore::Walk::new(&self.root);
        for result in walker {
            match result {
                Ok(entry) => {
                    if entry.file_type().map_or(false, |ft| ft.is_file()) {
                        if let Ok(rel) = entry.path().strip_prefix(&self.root) {
                            names.push(rel.to_string_lossy().replace("\\", "/"));
                        }
                    }
                }
                Err(_) => continue,
            }
        }
        Ok(names)
    }
}
