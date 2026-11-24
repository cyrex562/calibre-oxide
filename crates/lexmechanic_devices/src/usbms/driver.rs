use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::scanner::USBDevice;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const METADATA_CACHE: &str = "metadata.calibre";

#[derive(Debug, Serialize, Deserialize)]
pub struct BookMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub lpath: String,
    pub size: u64,
    #[serde(default)]
    pub uuid: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MetadataCache {
    #[serde(default)]
    pub books: Vec<BookMetadata>,
}

pub struct USBMSDevice {
    main_prefix: Option<PathBuf>,
    card_a_prefix: Option<PathBuf>,
    card_b_prefix: Option<PathBuf>,
    connected: bool,
    formats: Vec<String>,
}

impl USBMSDevice {
    pub fn new() -> Self {
        Self {
            main_prefix: None,
            card_a_prefix: None,
            card_b_prefix: None,
            connected: false,
            formats: vec!["epub".into(), "mobi".into(), "pdf".into(), "azw3".into()],
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        let mut device = Self::new();
        device.main_prefix = Some(path);
        device
    }

    pub fn get_prefix(&self, on_card: Option<&str>) -> Result<&Path> {
        match on_card {
            Some("carda") => self
                .card_a_prefix
                .as_deref()
                .context("Card A not available"),
            Some("cardb") => self
                .card_b_prefix
                .as_deref()
                .context("Card B not available"),
            _ => self
                .main_prefix
                .as_deref()
                .context("Main memory not available"),
        }
    }

    pub fn read_metadata_cache(&self, prefix: &Path) -> Result<MetadataCache> {
        let cache_path = prefix.join(METADATA_CACHE);
        if cache_path.exists() {
            let content =
                fs::read_to_string(&cache_path).context("Failed to read metadata cache")?;
            serde_json::from_str(&content).context("Failed to parse metadata cache")
        } else {
            Ok(MetadataCache { books: vec![] })
        }
    }

    pub fn write_metadata_cache(&self, prefix: &Path, cache: &MetadataCache) -> Result<()> {
        let cache_path = prefix.join(METADATA_CACHE);
        let content =
            serde_json::to_string_pretty(cache).context("Failed to serialize metadata cache")?;
        let mut file =
            fs::File::create(&cache_path).context("Failed to create metadata cache file")?;
        file.write_all(content.as_bytes())
            .context("Failed to write metadata cache")?;
        file.sync_all()?;
        Ok(())
    }

    pub fn scan_books(&self, prefix: &Path) -> Result<Vec<DeviceBook>> {
        let mut books = Vec::new();
        let mut cache = self
            .read_metadata_cache(prefix)
            .unwrap_or_else(|_| MetadataCache { books: vec![] });

        // Build a map of existing cached books
        let mut cache_map: HashMap<String, &BookMetadata> =
            cache.books.iter().map(|b| (b.lpath.clone(), b)).collect();

        // Scan filesystem
        if prefix.exists() {
            for entry in fs::read_dir(prefix)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        let ext_str = ext.to_string_lossy().to_lowercase();
                        if self.formats.contains(&ext_str) {
                            let file_name = path.file_name().unwrap().to_string_lossy();
                            let lpath = file_name.to_string();
                            let size = entry.metadata()?.len();

                            // Check if in cache
                            if let Some(cached) = cache_map.get(&lpath) {
                                books.push(DeviceBook {
                                    title: cached.title.clone(),
                                    authors: cached.authors.clone(),
                                    path: path.clone(),
                                    size,
                                });
                                cache_map.remove(&lpath);
                            } else {
                                // New book not in cache - use filename as title
                                let title = path.file_stem().unwrap().to_string_lossy().to_string();
                                books.push(DeviceBook {
                                    title: title.clone(),
                                    authors: vec!["Unknown".to_string()],
                                    path: path.clone(),
                                    size,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(books)
    }
}

impl Default for USBMSDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for USBMSDevice {
    fn can_handle(&self, _device: &USBDevice, _debug: bool) -> bool {
        // For USBMS, we accept any device - the actual filtering
        // happens at the scanner level
        true
    }

    fn open(&mut self, _device: &USBDevice, _library_uuid: Option<&str>) -> Result<()> {
        // For USBMS, the device is already "mounted" by the OS
        // We just need to verify the paths exist
        if let Some(ref prefix) = self.main_prefix {
            if !prefix.exists() {
                anyhow::bail!("Main device path does not exist: {:?}", prefix);
            }
        } else {
            anyhow::bail!("No device path configured");
        }

        self.connected = true;
        Ok(())
    }

    fn eject(&mut self) -> Result<()> {
        // For USBMS, ejection is handled by the OS
        // We just mark as disconnected
        self.connected = false;
        self.main_prefix = None;
        self.card_a_prefix = None;
        self.card_b_prefix = None;
        Ok(())
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "USB Mass Storage Device".to_string(),
            version: "1.0".to_string(),
            software_version: "1.0".to_string(),
            model: "Generic USBMS".to_string(),
        })
    }

    fn books(&self, on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        let prefix = self.get_prefix(on_card)?;
        self.scan_books(prefix)
    }

    fn upload_books(
        &mut self,
        files: &[PathBuf],
        names: &[String],
        on_card: Option<&str>,
    ) -> Result<()> {
        if files.len() != names.len() {
            anyhow::bail!("Files and names length mismatch");
        }

        let prefix = self.get_prefix(on_card)?.to_path_buf();

        // Ensure directory exists
        if !prefix.exists() {
            fs::create_dir_all(&prefix)?;
        }

        // Read existing cache
        let mut cache = self
            .read_metadata_cache(&prefix)
            .unwrap_or_else(|_| MetadataCache { books: vec![] });

        // Copy files and update cache
        for (file, name) in files.iter().zip(names.iter()) {
            let dest = prefix.join(name);
            fs::copy(file, &dest)
                .with_context(|| format!("Failed to copy {:?} to {:?}", file, dest))?;

            let size = fs::metadata(&dest)?.len();
            let title = Path::new(name)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();

            // Add or update in cache
            cache.books.retain(|b| b.lpath != *name);
            cache.books.push(BookMetadata {
                title,
                authors: vec!["Unknown".to_string()],
                lpath: name.clone(),
                size,
                uuid: None,
            });
        }

        // Write updated cache
        self.write_metadata_cache(&prefix, &cache)?;

        Ok(())
    }
}
