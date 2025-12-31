use calibre_devices::interface::Device;
use calibre_devices::scanner::USBDevice;
use calibre_devices::usbms::USBMSDevice;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_usbms_basic_flow() {
    // Create a temporary directory to simulate a device
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let device_path = temp_dir.path().to_path_buf();

    // Create a mock device
    let mut driver = USBMSDevice::with_path(device_path.clone());
    let usb_device = USBDevice::new(
        0x1234,
        0x5678,
        0x0100,
        "Test Vendor".into(),
        "Test Device".into(),
        "12345".into(),
    );

    // Open the device
    driver
        .open(&usb_device, None)
        .expect("Failed to open device");

    // Initially, no books
    let books = driver.books(None).expect("Failed to list books");
    assert_eq!(books.len(), 0);

    // Create a dummy book file
    let book_path = device_path.join("test_book.epub");
    let mut file = fs::File::create(&book_path).expect("Failed to create book file");
    file.write_all(b"dummy epub content")
        .expect("Failed to write book content");
    drop(file);

    // Upload books (simulating adding metadata)
    let files = vec![book_path.clone()];
    let names = vec!["uploaded_book.epub".to_string()];
    driver
        .upload_books(&files, &names, None)
        .expect("Failed to upload books");

    // Verify the book was copied
    let uploaded_path = device_path.join("uploaded_book.epub");
    assert!(uploaded_path.exists());

    // Verify metadata.calibre was created
    let metadata_path = device_path.join("metadata.calibre");
    assert!(metadata_path.exists());

    // List books again
    let books = driver.books(None).expect("Failed to list books");
    assert!(books.len() >= 1);

    // Find our uploaded book
    let uploaded_book = books
        .iter()
        .find(|b| b.path.file_name().unwrap() == "uploaded_book.epub");
    assert!(uploaded_book.is_some());
    let uploaded_book = uploaded_book.unwrap();
    assert_eq!(uploaded_book.title, "uploaded_book");

    // Eject
    driver.eject().expect("Failed to eject");
}

#[test]
fn test_usbms_metadata_persistence() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let device_path = temp_dir.path().to_path_buf();

    let mut driver = USBMSDevice::with_path(device_path.clone());
    let usb_device = USBDevice::new(
        0x1234,
        0x5678,
        0x0100,
        "Vendor".into(),
        "Device".into(),
        "123".into(),
    );

    driver.open(&usb_device, None).expect("Failed to open");

    // Create and upload a book
    let book_path = device_path.join("source.epub");
    fs::write(&book_path, b"content").expect("Failed to write");

    driver
        .upload_books(&[book_path], &["my_book.epub".to_string()], None)
        .expect("Failed to upload");

    // Close and reopen (simulating disconnect/reconnect)
    driver.eject().expect("Failed to eject");

    let mut driver2 = USBMSDevice::with_path(device_path.clone());
    driver2.open(&usb_device, None).expect("Failed to reopen");

    // Books should still be listed from cache
    let books = driver2.books(None).expect("Failed to list books");
    assert!(books.iter().any(|b| b.title == "my_book"));
}

#[test]
fn test_usbms_device_info() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let device_path = temp_dir.path().to_path_buf();

    let driver = USBMSDevice::with_path(device_path);
    let info = driver
        .get_device_information()
        .expect("Failed to get device info");

    assert_eq!(info.name, "USB Mass Storage Device");
    assert!(!info.version.is_empty());
}
