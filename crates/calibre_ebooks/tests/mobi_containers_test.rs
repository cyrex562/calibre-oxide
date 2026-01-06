use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::containers::{find_imgtype, Container};

#[test]
fn test_find_imgtype() {
    assert_eq!(find_imgtype(b"\xFF\xD8\xFF\xE0"), Some("jpeg"));
    assert_eq!(find_imgtype(b"\x89PNG\r\n\x1A\n"), Some("png"));
    assert_eq!(find_imgtype(b"GIF89a"), Some("gif"));
    assert_eq!(find_imgtype(b"Garbage"), None);
}

#[test]
fn test_container_detect_image() {
    // Construct a mock MOBI header/record 0 that has EXTH
    let mut data = vec![0u8; 48];
    data.extend_from_slice(b"EXTH"); // at 48
                                     // EXTH header: len(4), count(4)
                                     // len includes identifiers etc. 12 + records.
                                     // Record: type(4), len(4), content.
                                     // We need type 539, content "application/image".
    let content = b"application/image";
    let rec_len = 8 + content.len() as u32;
    let exth_len = 12 + rec_len;

    data.write_u32::<BigEndian>(exth_len).unwrap(); // len at 52
    data.write_u32::<BigEndian>(1).unwrap(); // count at 56

    // Record start at 60
    data.write_u32::<BigEndian>(539).unwrap(); // type
    data.write_u32::<BigEndian>(rec_len).unwrap(); // len
    data.extend_from_slice(content);

    let container = Container::new(&data);
    assert!(container.is_image_container);
}

#[test]
fn test_container_not_image() {
    let data = vec![0u8; 100];
    let container = Container::new(&data);
    assert!(!container.is_image_container);
}

#[test]
fn test_load_image() {
    // Create container
    let mut data = vec![0u8; 48];
    data.extend_from_slice(b"EXTH");
    let content = b"application/image";
    let rec_len = 8 + content.len() as u32;
    let exth_len = 12 + rec_len;
    data.write_u32::<BigEndian>(exth_len).unwrap();
    data.write_u32::<BigEndian>(1).unwrap();
    data.write_u32::<BigEndian>(539).unwrap();
    data.write_u32::<BigEndian>(rec_len).unwrap();
    data.extend_from_slice(content);

    let mut container = Container::new(&data);

    // Mock image record: 12 bytes prefix + PNG data
    let mut img_rec = vec![0u8; 12];
    img_rec.extend_from_slice(b"\x89PNG\r\n\x1A\nSomeData");

    let (data_out, type_out) = container.load_image(&img_rec);
    assert!(data_out.is_some());
    assert_eq!(type_out, Some("png"));
    assert_eq!(container.resource_index, 1);
}
