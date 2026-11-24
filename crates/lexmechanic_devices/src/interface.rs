use anyhow::Result;
use std::path::{Path, PathBuf};

/// Metadata about a book to be used for device operations
#[allow(dead_code)]
pub struct DeviceBook {
    pub title: String,
    pub authors: Vec<String>,
    pub path: PathBuf,
    pub size: u64,
    // ... other metadata
}

/// Information about the connected device
#[allow(dead_code)]
pub struct DeviceInfo {
    pub name: String,
    pub version: String,
    pub software_version: String,
    pub model: String,
}

/// The main trait that all device drivers must implement.
/// Corresponds to `DevicePlugin` in Python.
pub trait Device {
    /// Return true if this driver can handle the detected device
    fn can_handle(&self, device_info: &crate::scanner::USBDevice, debug: bool) -> bool;

    /// Perform device-specific initialization.
    fn open(&mut self, device: &crate::scanner::USBDevice, library_uuid: Option<&str>) -> Result<()>;

    /// Un-mount / eject the device.
    fn eject(&mut self) -> Result<()>;

    /// Get information about the device.
    fn get_device_information(&self) -> Result<DeviceInfo>;

    /// Return a list of e-books on the device.
    /// `on_card` can be None, "carda", "cardb" etc.
    fn books(&self, on_card: Option<&str>) -> Result<Vec<DeviceBook>>;

    /// Upload books to the device.
    fn upload_books(
        &mut self,
        files: &[PathBuf],
        names: &[String],
        on_card: Option<&str>,
    ) -> Result<()>;
    
    // Add other methods as needed: delete_books, get_file, etc.
}
