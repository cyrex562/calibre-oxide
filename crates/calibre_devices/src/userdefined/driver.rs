use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::scanner::USBDevice;
use crate::usbms::driver::USBMSDevice;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct UserDefinedConfig {
    pub vendor_id: u16,
    pub product_id: u16,
    pub vendor_name: Option<String>,
    pub main_memory_path: Option<PathBuf>,
    pub card_a_path: Option<PathBuf>,
}

pub struct UserDefinedDevice {
    config: UserDefinedConfig,
    usbms: USBMSDevice,
}

impl UserDefinedDevice {
    pub fn new(config: UserDefinedConfig) -> Self {
        UserDefinedDevice {
            config,
            usbms: USBMSDevice::new(),
        }
    }

    pub fn set_paths(&mut self, main: Option<PathBuf>, card_a: Option<PathBuf>) {
        if let Some(path) = main {
            self.usbms = USBMSDevice::with_path(path);
        }
        // Ideally USBMSDevice would support setting card paths too, but for now we rely on its basic structure
        // If USBMSDevice gets card support, we'd hook it up here using card_a
    }
}

impl Device for UserDefinedDevice {
    fn can_handle(&self, device: &USBDevice, _debug: bool) -> bool {
        if device.vendor_id == self.config.vendor_id && device.product_id == self.config.product_id
        {
            return true;
        }
        false
    }

    fn open(&mut self, _device: &USBDevice, _library_uuid: Option<&str>) -> Result<()> {
        // In a real scenario, we might auto-detect the mount point from the USBDevice
        // For this port, we assume paths are pre-configured or passed in
        if let Some(ref path) = self.config.main_memory_path {
            self.usbms = USBMSDevice::with_path(path.clone());
        } else {
            // Fallback or error if no path configured?
            // For now, let's allow it to open but operations might fail if path not set
        }

        // Delegate open to USBMS (checks path existence)
        // USBMSDevice::open expects to verify specific paths if set
        // However, USBMSDevice::open signature is: fn open(&mut self, _device: &USBDevice, ...)
        // And it checks self.main_prefix

        // Since we re-created self.usbms with the path in set_paths or above, we can just call open
        self.usbms.open(_device, _library_uuid)
    }

    fn eject(&mut self) -> Result<()> {
        self.usbms.eject()
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "User Defined Device".to_string(),
            version: "1.0.0".to_string(),
            software_version: "1.0.0".to_string(),
            model: "User Configuration".to_string(),
        })
    }

    fn books(&self, on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        self.usbms.books(on_card)
    }

    fn upload_books(
        &mut self,
        files: &[PathBuf],
        names: &[String],
        on_card: Option<&str>,
    ) -> Result<()> {
        self.usbms.upload_books(files, names, on_card)
    }
}
