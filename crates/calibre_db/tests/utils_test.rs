use calibre_db::utils::{force_to_bool, fuzzy_title, ThumbnailCache};
use std::fs;

#[test]
fn test_force_to_bool() {
    assert_eq!(force_to_bool("yes"), Some(true));
    assert_eq!(force_to_bool("YES"), Some(true));
    assert_eq!(force_to_bool("1"), Some(true));
    assert_eq!(force_to_bool("checked"), Some(true));

    assert_eq!(force_to_bool("no"), Some(false));
    assert_eq!(force_to_bool("false"), Some(false));
    assert_eq!(force_to_bool("0"), Some(false));
    assert_eq!(force_to_bool("unchecked"), Some(false));

    assert_eq!(force_to_bool("apple"), None);
}

#[test]
fn test_fuzzy_title() {
    assert_eq!(fuzzy_title("Hello-World"), "hello world");
    assert_eq!(fuzzy_title("Hello_World"), "hello world");
    assert_eq!(fuzzy_title("Title: Subtitle"), "title subtitle");
    assert_eq!(fuzzy_title("  Space  "), "space");
}

#[test]
fn test_thumbnail_cache() {
    let temp_dir = std::env::temp_dir().join("calibre_oxide_thumb_test_integ");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    let mut cache = ThumbnailCache::new(temp_dir.clone(), 1, (100, 100)); // 1MB max

    // Insert
    let data = vec![0u8; 1024]; // 1KB
    cache.insert(1, 100.0, &data);

    // Get
    let res = cache.get(1);
    assert!(res.is_some());
    assert_eq!(res.unwrap().0.len(), 1024);

    // Invalidate
    cache.invalidate(&[1]);
    assert!(cache.get(1).is_none());

    fs::remove_dir_all(&temp_dir).unwrap();
}
