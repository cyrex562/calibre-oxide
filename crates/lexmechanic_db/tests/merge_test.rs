//! Merging books in a library calibre made: every custom-column type, and
//! books in calibre's own `Author/Title (id)` folders.

mod common;

use lexmechanic_db::cache::Cache;
use lexmechanic_db::merge::{merge_books, MergeMode, MergeOptions};
use common::*;

const LABELS: [&str; 9] = ["genre", "notes", "saga", "mood", "pages", "score", "stars", "read", "finished"];

fn open() -> (CalibreFixture, Cache) {
    let fixture = build_calibre_library();
    let cache = Cache::new(fixture.root()).unwrap();
    (fixture, cache)
}

fn value(cache: &Cache, id: i32, label: &str) -> Option<String> {
    cache.get_custom_column_value(id, label).unwrap().filter(|v| !v.is_empty())
}

fn all(cache: &Cache, id: i32) -> Vec<Option<String>> {
    LABELS.iter().map(|l| value(cache, id, l)).collect()
}

fn keep(mode: MergeMode) -> MergeOptions {
    MergeOptions { mode, ..Default::default() }
}

#[test]
fn a_bare_destination_takes_every_custom_column_type_from_the_first_source_that_has_it() {
    let (_fx, cache) = open();
    let hobbit = all(&cache, 1);
    assert!(hobbit.iter().all(Option::is_some), "the fixture's first book has all nine: {hobbit:?}");
    assert!(all(&cache, 4).iter().all(Option::is_none));
    let saga_index = cache.get_custom_series_index(1, "saga").unwrap();

    merge_books(&cache, 4, &[1, 2], &keep(MergeMode::MergeKeepSources)).unwrap();

    // genre (text, multiple) is a union of what the sources have.
    assert_eq!(all(&cache, 4), hobbit, "one value per type, from book 1");
    assert_eq!(cache.get_custom_series_index(4, "saga").unwrap(), saga_index, "a series brings its index");
    assert_eq!(all(&cache, 1), hobbit, "keep-sources: the source is unchanged");
}

#[test]
fn a_destination_that_has_a_value_keeps_it_and_multiple_text_is_unioned() {
    let (_fx, cache) = open();
    let before = all(&cache, 1);
    merge_books(&cache, 1, &[2, 3], &keep(MergeMode::MergeKeepSources)).unwrap();
    let after = all(&cache, 1);

    // Everything that was set stays...
    for (i, label) in LABELS.iter().enumerate() {
        if *label != "genre" {
            assert_eq!(after[i], before[i], "{label}");
        }
    }
    // ...except the multiple-valued one, which gains the other books' values.
    let genre = after[0].clone().unwrap();
    for want in ["High Fantasy", "Classic", "Comic Fantasy"] {
        assert!(genre.contains(want), "{genre}");
    }
    assert_eq!(genre.matches("High Fantasy").count(), 1, "no duplicates: {genre}");
}

#[test]
fn only_empty_columns_are_filled_and_custom_comments_are_appended() {
    let (_fx, cache) = open();
    // Book 2 has no notes (comments) and no finished date; book 3 has a date.
    cache.set_custom_column_value(2, "notes", "<p>mine</p>").unwrap();
    merge_books(&cache, 2, &[3, 1], &keep(MergeMode::MergeKeepSources)).unwrap();

    assert_eq!(value(&cache, 2, "notes").unwrap(), format!("<p>mine</p>\n\n{}", value(&cache, 1, "notes").unwrap()));
    assert_eq!(value(&cache, 2, "finished"), value(&cache, 3, "finished"), "first source with a date");
    assert_eq!(value(&cache, 2, "pages").as_deref(), Some("423"), "an existing int is kept");
    assert_eq!(value(&cache, 2, "mood").as_deref(), Some("Tense"), "an existing enumeration is kept");
}

#[test]
fn zero_is_a_value_for_an_int_or_float_column() {
    let (_fx, cache) = open();
    // Book 3 has pages = 0 and score = 0.0: set, not empty.
    let (pages, score) = (value(&cache, 3, "pages"), value(&cache, 3, "score"));
    assert!(pages.is_some() && score.is_some());
    merge_books(&cache, 3, &[1], &keep(MergeMode::MergeKeepSources)).unwrap();
    assert_eq!(value(&cache, 3, "pages"), pages);
    assert_eq!(value(&cache, 3, "score"), score);
}

#[test]
fn merging_into_a_calibre_folder_moves_the_formats_and_trashes_the_original_folder() {
    let (fx, cache) = open();
    // 1 (The Hobbit) has EPUB and TXT; 4 has only EPUB with different bytes.
    let hobbit_txt = BOOKS[0].format_path(fx.root(), "TXT");
    let hobbit_epub = BOOKS[0].format_path(fx.root(), "EPUB");
    assert!(hobbit_txt.is_file() && hobbit_epub.is_file());
    let bare_epub_before = std::fs::read(BOOKS[3].format_path(fx.root(), "EPUB")).unwrap();
    let report = merge_books(&cache, 4, &[1], &MergeOptions::default()).unwrap();

    let mut formats: Vec<String> = cache.format_file_info(4).unwrap().into_iter().map(|f| f.fmt.to_uppercase()).collect();
    formats.sort();
    assert_eq!(formats, vec!["EPUB", "TXT"]);
    let landed = cache.format_file_info(4).unwrap().into_iter().find(|f| f.fmt.eq_ignore_ascii_case("txt")).unwrap().path;
    assert_eq!(std::fs::read(&landed).unwrap(), BOOKS[0].format_bytes("TXT"));
    assert!(landed.starts_with(fx.root().join(BOOKS[3].rel_dir())), "{}", landed.display());
    assert_eq!(std::fs::read(BOOKS[3].format_path(fx.root(), "EPUB")).unwrap(), bare_epub_before, "the destination's EPUB wins");
    assert!(!hobbit_txt.exists() && !hobbit_epub.exists(), "originals went to the wastebasket: {report:?}");
    assert!(!fx.root().join(BOOKS[0].rel_dir()).exists(), "its emptied folder is removed");
    assert!(cache.field_for(1, "path").unwrap().is_none());
    // The cover followed, too: the destination had none.
    assert_eq!(report.cover_from, Some(1));
    assert_eq!(std::fs::read(lexmechanic_db::covers::cover_path(&cache, 4).unwrap()).unwrap(), BOOKS[0].cover_bytes());
}
