use calibre_devices::interface::Device;
use calibre_devices::scanner::USBDevice;
use calibre_devices::userdefined::{UserDefinedConfig, UserDefinedDevice};
use std::fs;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_userdefined_can_handle() {
    let config = UserDefinedConfig {
        vendor_id: 0x1234,
        product_id: 0x5678,
        vendor_name: None,
        main_memory_path: None,
        card_a_path: None,
    };
    let driver = UserDefinedDevice::new(config);

    // Matching device
    let usb_device_match = USBDevice::new(
        0x1234,
        0x5678,
        0x0100,
        "Vendor".into(),
        "Model".into(),
        "Serial".into(),
    );
    assert!(driver.can_handle(&usb_device_match, false));

    // Mismatching device
    let usb_device_mismatch = USBDevice::new(
        0x9999,
        0x8888,
        0x0100,
        "Another".into(),
        "Device".into(),
        "S2".into(),
    );
    assert!(!driver.can_handle(&usb_device_mismatch, false));
}

#[test]
fn test_userdefined_basic_flow() {
    // Mock user config with temp dir
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let device_path = temp_dir.path().to_path_buf();

    // Source dir (separate from device)
    let source_dir = TempDir::new().expect("Failed to create source dir");
    let source_path = source_dir.path().to_path_buf();

    let config = UserDefinedConfig {
        vendor_id: 0xAAAA,
        product_id: 0xBBBB,
        vendor_name: Some("CustomVendor".into()),
        main_memory_path: Some(device_path.clone()),
        card_a_path: None,
    };

    let mut driver = UserDefinedDevice::new(config);

    // Mock USB device presence
    let usb_device = USBDevice::new(
        0xAAAA,
        0xBBBB,
        0x0100,
        "CustomVendor".into(),
        "CustomDrive".into(),
        "123".into(),
    );

    // Open
    driver
        .open(&usb_device, None)
        .expect("Failed to open device");

    // Check Info
    let info = driver.get_device_information().unwrap();
    assert_eq!(info.name, "User Defined Device");

    // Books
    let books = driver.books(None).expect("Books failed");
    assert!(books.is_empty());

    // Upload
    let book_path = source_path.join("test.epub");
    let mut f = fs::File::create(&book_path).unwrap();
    f.write_all(b"content").unwrap();
    drop(f);

    driver
        .upload_books(&[book_path], &["uploaded.epub".into()], None)
        .expect("Upload failed");

    // Verify
    let books_after = driver.books(None).unwrap();
    assert_eq!(books_after.len(), 1);
    assert_eq!(books_after[0].title, "uploaded");
}
