use calibre_ebooks::oeb::container::*;
use tempfile::TempDir;

#[test]
fn test_dir_container() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();

    let mut container = DirContainer::new(root);

    // Write
    let data = b"Hello, World!";
    container.write("test.txt", data).unwrap();
    assert!(container.exists("test.txt"));

    // Read
    let read_data = container.read("test.txt").unwrap();
    assert_eq!(read_data, data);

    // Namelist
    let names = container.namelist().unwrap();
    assert!(names.contains(&"test.txt".to_string()));

    // Subdir write
    container.write("sub/file.txt", b"Sub").unwrap();
    assert!(container.exists("sub/file.txt"));

    let names_sub = container.namelist().unwrap();
    assert!(names_sub.contains(&"sub/file.txt".to_string()));
}
