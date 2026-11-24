use crate::interface::{Device, DeviceBook, DeviceInfo};
use crate::scanner::USBDevice;
use crate::usbms::driver::USBMSDevice;
use anyhow::Result;
use std::path::PathBuf;

pub struct KoboDevice {
    usbms: USBMSDevice,
}

impl KoboDevice {
    pub fn new(usbms: USBMSDevice) -> Self {
        Self { usbms }
    }
}

impl Default for KoboDevice {
    fn default() -> Self {
        Self::new(USBMSDevice::new())
    }
}

impl Device for KoboDevice {
    fn can_handle(&self, device_info: &USBDevice, _debug: bool) -> bool {
        let vendor_id = 0x2237;
        let product_ids = vec![0x4165, 0x4161, 0x4162];

        if device_info.vendor_id == vendor_id && product_ids.contains(&device_info.product_id) {
            return true;
        }
        false
    }

    fn open(&mut self, device: &USBDevice, library_uuid: Option<&str>) -> Result<()> {
        self.usbms.open(device, library_uuid)
    }

    fn eject(&mut self) -> Result<()> {
        self.usbms.eject()
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Kobo eReader".to_string(),
            version: "1.0.0".to_string(),
            software_version: "1.0.0".to_string(),
            model: "Generic Kobo".to_string(),
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
