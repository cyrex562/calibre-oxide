use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::scanner::USBDevice;
use anyhow::Result;
use std::path::PathBuf;

pub struct NookDevice;

impl NookDevice {
    pub fn new() -> Self {
        NookDevice
    }
}

impl Device for NookDevice {
    fn can_handle(&self, _device: &USBDevice, _debug: bool) -> bool {
        // Stub
        false
    }

    fn open(&mut self, _device: &USBDevice, _library_uuid: Option<&str>) -> Result<()> {
        anyhow::bail!("NookDevice support is a stub")
    }

    fn eject(&mut self) -> Result<()> {
        Ok(())
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Nook Device".to_string(),
            version: "0.0".to_string(),
            software_version: "0.0".to_string(),
            model: "Generic Nook".to_string(),
        })
    }

    fn books(&self, _on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        Ok(vec![])
    }

    fn upload_books(
        &mut self,
        _files: &[PathBuf],
        _names: &[String],
        _on_card: Option<&str>,
    ) -> Result<()> {
        anyhow::bail!("Not supported")
    }
}
