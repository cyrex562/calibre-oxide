use anyhow::Result;
use calibre_devices::interface::{Device, DeviceBook, DeviceInfo};
use calibre_devices::scanner::USBDevice;
use std::path::PathBuf;

// Mock Device Implementation
struct MockDevice {
    connected: bool,
}

impl Device for MockDevice {
    fn can_handle(&self, _device: &USBDevice, _debug: bool) -> bool {
        true
    }

    fn open(&mut self, _device: &USBDevice, _lib_uuid: Option<&str>) -> Result<()> {
        self.connected = true;
        Ok(())
    }

    fn eject(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    fn get_device_information(&self) -> Result<DeviceInfo> {
        Ok(DeviceInfo {
            name: "Mock Device".into(),
            version: "1.0".into(),
            software_version: "1.0".into(),
            model: "Test Model".into(),
        })
    }

    fn books(&self, _on_card: Option<&str>) -> Result<Vec<DeviceBook>> {
        Ok(vec![DeviceBook {
            title: "Test Book".into(),
            authors: vec!["Author".into()],
            path: PathBuf::from("books/test.epub"),
            size: 1024,
        }])
    }

    fn upload_books(
        &mut self,
        _files: &[PathBuf],
        _names: &[String],
        _on_card: Option<&str>,
    ) -> Result<()> {
        Ok(())
    }
}

#[test]
fn test_device_trait_flow() {
    let mut driver = MockDevice { connected: false };
    let device = USBDevice::new(
        0x1234,
        0x5678,
        0x0100,
        "Vendor".into(),
        "Model".into(),
        "Serial".into(),
    );

    assert!(driver.can_handle(&device, false));

    driver.open(&device, None).expect("Failed to open");
    assert!(driver.connected);

    let info = driver.get_device_information().expect("Failed to get info");
    assert_eq!(info.name, "Mock Device");

    let books = driver.books(None).expect("Failed to list books");
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Test Book");

    driver.eject().expect("Failed to eject");
    assert!(!driver.connected);
}
