use calibre_devices::interface::Device;
use calibre_devices::nook::NookDevice;
use calibre_devices::scanner::USBDevice;
use calibre_devices::smart_device_app::SmartDevice;
use calibre_devices::userdefined::{UserDefinedConfig, UserDefinedDevice};

#[test]
fn test_device_stubs() {
    let dummy_usb = USBDevice {
        vendor_id: 0,
        product_id: 0,
        bcd: 0,
        manufacturer: "".to_string(),
        model: "".to_string(),
        serial: "".to_string(),
    };

    let mut smart = SmartDevice::new();
    assert!(!smart.can_handle(&dummy_usb, false));
    assert!(smart.open(&dummy_usb, None).is_err());
    assert_eq!(
        smart.get_device_information().unwrap().name,
        "Smart Device App"
    );

    // Non-zero ids so this config genuinely doesn't match `dummy_usb`
    // (0/0) -- a config left at 0/0 would spuriously match it, since
    // `can_handle` is a plain vendor/product id equality check.
    let mut user = UserDefinedDevice::new(UserDefinedConfig {
        vendor_id: 1,
        product_id: 1,
        vendor_name: None,
        main_memory_path: None,
        card_a_path: None,
    });
    assert!(!user.can_handle(&dummy_usb, false));
    assert!(user.open(&dummy_usb, None).is_err());
    assert_eq!(
        user.get_device_information().unwrap().name,
        "User Defined Device"
    );

    let mut nook = NookDevice::new();
    assert!(!nook.can_handle(&dummy_usb, false));
    assert!(nook.open(&dummy_usb, None).is_err());
    assert_eq!(nook.get_device_information().unwrap().name, "Nook Device");
}
