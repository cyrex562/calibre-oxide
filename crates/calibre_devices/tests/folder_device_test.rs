use calibre_devices::folder_device::FolderDevice;
use calibre_devices::interface::Device;
use calibre_devices::scanner::USBDevice;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn test_folder_device_lifecycle() {
    let temp_dir = tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    // Create a dummy file in root
    let book_path = root.join("test_book.epub");
    let mut f = File::create(&book_path).unwrap();
    f.write_all(b"content").unwrap();

    let mut dev = FolderDevice::with_path(root.clone());

    // Mock open (scanner device ignored)
    let dummy_usb = USBDevice {
        vendor_id: 0,
        product_id: 0,
        bcd: 0,
        manufacturer: "".to_string(),
        model: "".to_string(),
        serial: "".to_string(),
    };
    dev.open(&dummy_usb, None).expect("Open failed");

    // List books
    let books = dev.books(None).expect("Failed to list books");
    assert!(!books.is_empty());
    assert_eq!(books[0].title, "test_book.epub");

    // Upload book
    let new_book_path = temp_dir.path().join("upload.epub");
    File::create(&new_book_path)
        .unwrap()
        .write_all(b"new")
        .unwrap();

    dev.upload_books(&[new_book_path], &["uploaded.epub".to_string()], None)
        .expect("Upload failed");

    assert!(root.join("uploaded.epub").exists());
}
