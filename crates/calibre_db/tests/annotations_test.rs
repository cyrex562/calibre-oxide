use calibre_db::annotations::{sort_annotations_by_timestamp, Annotation, Bookmark, Highlight};

#[test]
fn test_annotation_sorting() {
    let b1 = Annotation::Bookmark(Bookmark {
        title: "B1".to_string(),
        timestamp: "2023-01-01T10:00:00Z".to_string(),
        pos: "cfi1".to_string(),
        pos_type: "epubcfi".to_string(),
    });

    let h1 = Annotation::Highlight(Highlight {
        uuid: "h1".to_string(),
        timestamp: "2023-01-02T10:00:00Z".to_string(),
        start_cfi: "cfi2".to_string(),
        end_cfi: "cfi3".to_string(),
        highlighted_text: None,
        notes: None,
    });

    let mut list = vec![b1.clone(), h1.clone()];
    sort_annotations_by_timestamp(&mut list);

    // Should be descending: H1 (Jan 2) then B1 (Jan 1)
    assert_eq!(list[0], h1);
    assert_eq!(list[1], b1);
}
