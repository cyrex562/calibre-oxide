use lexmechanic_devices::android::AndroidDevice;
use lexmechanic_devices::interface::Device;
use lexmechanic_devices::scanner::USBDevice;

#[test]
fn test_android_stub() {
    let mut dev = AndroidDevice::new();
    let dummy_usb = USBDevice {
        vendor_id: 0,
        product_id: 0,
        bcd: 0,
        manufacturer: "".to_string(),
        model: "".to_string(),
        serial: "".to_string(),
    };

    assert!(!dev.can_handle(&dummy_usb, false));
    assert!(dev.open(&dummy_usb, None).is_err()); // Should be stubbed
}
