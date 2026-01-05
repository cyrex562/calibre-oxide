use calibre_devices::interface::Device;
use calibre_devices::kindle::driver::KindleDevice;
use std::path::PathBuf;

#[test]
fn test_kindle_device_info() {
    let device = KindleDevice::new();
    let info = device
        .get_device_information()
        .expect("Failed to get device info");
    assert_eq!(info.name, "Amazon Kindle");
    assert_eq!(info.model, "Kindle");
}

#[test]
fn test_kindle_vendor_id() {
    let device = KindleDevice::new();
    let usb_device = calibre_devices::scanner::USBDevice {
        vendor_id: 0x1949,
        product_id: 0x0001,
        bcd: 0x100,
        manufacturer: "Amazon".to_string(),
        model: "Kindle".to_string(),
        serial: "123".to_string(),
    };
    assert!(device.can_handle(&usb_device, false));

    let other_device = calibre_devices::scanner::USBDevice {
        vendor_id: 0x1234,
        ..usb_device
    };
    assert!(!device.can_handle(&other_device, false));
}

#[test]
fn test_kindle_eject() {
    let mut device = KindleDevice::new();
    // Eject should succeed even if not connected
    assert!(device.eject().is_ok());
}
