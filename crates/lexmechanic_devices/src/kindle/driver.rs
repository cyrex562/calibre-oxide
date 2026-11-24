use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::kindle::apnx::APNXBuilder;
// use crate::kindle::bookmark::Bookmark; // Unused for now
use crate::scanner::USBDevice;
use crate::usbms::driver::USBMSDevice;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct KindleDevice {
    usbms: USBMSDevice,
}

impl KindleDevice {
    pub fn new() -> Self {
        Self {
            usbms: USBMSDevice::new(),
        }
    }
}

impl Default for KindleDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for KindleDevice {
    fn can_handle(&self, device: &USBDevice, _debug: bool) -> bool {
        // Kindle Vendor ID is 0x1949
        device.vendor_id == 0x1949
    }

    fn open(&mut self, device: &USBDevice, library_uuid: Option<&str>) -> Result<()> {
        self.usbms.open(device, library_uuid)
    }

    fn eject(&mut self) -> Result<()> {
        self.usbms.eject()
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Amazon Kindle".to_string(),
            version: "1.0".to_string(),
            software_version: "1.0".to_string(),
            model: "Kindle".to_string(),
        })
    }

    fn books(&self, on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        // Kindle stores books in 'documents' folder usually, but USBMS implementation
        // scans the prefix provided in `open`.
        // If `usbms.open` sets the root as prefix, `scan_books` scans root.
        // We might need to adjust the prefix to be `.../documents`.
        // However, USBMSDevice implementation is generic.
        // Let's assume for now the user (or scanner) provides the mount point,
        // and we scan that. Kindle often has books in root too or documents.
        // To strictly follow Kindle structure, we should scan `documents`.

        let prefix = self.usbms.get_prefix(on_card)?;
        let documents_path = prefix.join("documents");

        // If documents exists, scan it. Else scan root?
        if documents_path.exists() {
            self.usbms.scan_books(&documents_path)
        } else {
            self.usbms.scan_books(prefix)
        }
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

        let prefix = self.usbms.get_prefix(on_card)?.to_path_buf();
        // Kindle usually puts books in 'documents'
        let target_dir = prefix.join("documents");
        if !target_dir.exists() {
            std::fs::create_dir_all(&target_dir)?;
        }

        let mut cache = self
            .usbms
            .read_metadata_cache(&target_dir)
            .unwrap_or_else(|_| crate::usbms::driver::MetadataCache { books: vec![] }); // Now public

        for (file, name) in files.iter().zip(names.iter()) {
            let dest = target_dir.join(name);
            std::fs::copy(file, &dest)
                .with_context(|| format!("Failed to copy {:?} to {:?}", file, dest))?;

            let size = std::fs::metadata(&dest)?.len();
            let title = Path::new(name)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();

            // Update cache
            cache.books.retain(|b| b.lpath != *name);
            cache.books.push(crate::usbms::driver::BookMetadata {
                title,
                authors: vec!["Unknown".to_string()],
                lpath: name.clone(),
                size,
                uuid: None,
            });

            // Generate APNX if it's a MOBI/AZW3
            // APNX path is dest + ".apnx"? No, usually basename + ".apnx"
            // Python: `apnx_path = f'{os.path.join(path, filename)}.apnx'` where filename includes extension?
            // "filename" passed to upload_apnx is the basename.
            // If dest is "foo.mobi", apnx is "foo.apnx" or "foo.mobi.apnx"?
            // Python: `apnx_path = f'{os.path.join(path, filename)}.apnx'`
            // If filename="book.mobi", apnx="book.mobi.apnx".
            // However, typical Kindle sidecar is "book.sdr" dir.
            // Python `upload_apnx`: `apnx_path = f'{os.path.join(path, filename)}.apnx'`
            // It seems to append .apnx to the full filename.

            let ext = dest
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ["mobi", "azw3", "azw"].contains(&ext.as_str()) {
                let apnx_path = if let Some(file_name) = dest.file_name() {
                    let mut p = dest.parent().unwrap().join(file_name);
                    // The logic says `filename + .apnx`.
                    let mut fname = file_name.to_os_string();
                    fname.push(".apnx");
                    dest.with_file_name(fname)
                } else {
                    dest.with_extension("apnx") // Fallback
                };

                let builder = APNXBuilder::new();
                if let Err(e) = builder.write_apnx(&dest, &apnx_path) {
                    eprintln!("Failed to generate APNX for {:?}: {:?}", dest, e);
                    // Don't fail the upload just for APNX
                }
            }
        }

        self.usbms.write_metadata_cache(&target_dir, &cache)?;
        Ok(())
    }
}
