use calibre_devices::interface::Device;
use calibre_devices::kobo::driver::KoboDevice;
use calibre_devices::scanner::USBDevice;

#[test]
fn test_can_handle_kobo() {
    let device = KoboDevice::default();

    let kobo_touch = USBDevice::new(
        0x2237,
        0x4165,
        0x0110,
        "Kobo".to_string(),
        "eReader".to_string(),
        "123".to_string(),
    );

    let kobo_wifi = USBDevice::new(
        0x2237,
        0x4161,
        0x0110,
        "Kobo".to_string(),
        "WiFi".to_string(),
        "123".to_string(),
    );

    let unknown_device = USBDevice::new(
        0x1234,
        0x5678,
        0x0000,
        "Unknown".to_string(),
        "Device".to_string(),
        "000".to_string(),
    );

    assert!(device.can_handle(&kobo_touch, false));
    assert!(device.can_handle(&kobo_wifi, false));
    assert!(!device.can_handle(&unknown_device, false));
}
