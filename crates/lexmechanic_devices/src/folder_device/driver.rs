use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::scanner::USBDevice;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub struct FolderDevice {
    base_path: Option<PathBuf>,
    connected: bool,
}

impl FolderDevice {
    pub fn new() -> Self {
        FolderDevice {
            base_path: None,
            connected: false,
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        FolderDevice {
            base_path: Some(path),
            connected: false,
        }
    }
}

impl Device for FolderDevice {
    fn can_handle(&self, _device: &USBDevice, _debug: bool) -> bool {
        // Folder device is handled explicitly by path, not by USB scanning
        false
    }

    fn open(&mut self, _device: &USBDevice, _library_uuid: Option<&str>) -> Result<()> {
        if let Some(ref path) = self.base_path {
            if path.exists() {
                self.connected = true;
                Ok(())
            } else {
                anyhow::bail!("Folder path does not exist: {:?}", path)
            }
        } else {
            anyhow::bail!("No folder path configured")
        }
    }

    fn eject(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Folder Device".to_string(),
            version: "1.0".to_string(),
            software_version: "1.0".to_string(),
            model: "Directory".to_string(),
        })
    }

    fn books(&self, _on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        let mut books = Vec::new();
        if let Some(ref path) = self.base_path {
            if path.exists() {
                for entry in fs::read_dir(path)? {
                    let entry = entry?;
                    let p = entry.path();
                    if p.is_file() {
                        let name = p.file_name().unwrap().to_string_lossy().to_string();
                        let size = entry.metadata()?.len();
                        books.push(DeviceBook {
                            title: name.clone(), // Minimal impl: use filename as title
                            authors: vec!["Unknown".to_string()],
                            path: p,
                            size,
                        });
                    }
                }
            }
        }
        Ok(books)
    }

    fn upload_books(
        &mut self,
        files: &[PathBuf],
        names: &[String],
        _on_card: Option<&str>,
    ) -> Result<()> {
        let base = self.base_path.as_ref().context("Not connected")?;
        for (file, name) in files.iter().zip(names.iter()) {
            let dest = base.join(name);
            fs::copy(file, dest)?;
        }
        Ok(())
    }
}
