//! Port of `old_src/src/calibre/db/categories.py`'s `get_categories`
//! (issue #220, a #201 follow-up): the calibre "tag browser" sidebar
//! data.
//!
//! # Scope of this pass
//!
//! Upstream's real `get_categories` is field-metadata-driven (iterates
//! every category-flagged field, including custom columns and
//! composite fields, via `find_categories`/`create_tag_class`), and
//! also builds composite categories, user categories (with grouped-
//! search-term consolidation), hierarchical-category dotted-name
//! handling, a legacy duplicate-rating-id consolidation pass, and a
//! `search` category from saved searches. None of that is possible
//! without a `field_metadata` system (this crate doesn't have one --
//! same recurring gap as #210/#214/#216/#218) or virtual
//! libraries/saved searches (also not ported).
//!
//! What's real here instead: a fixed category list matching
//! `Cache::field_for`'s own standard-field set --
//! `authors`/`tags`/`series`/`publisher`/`languages` -- each with a
//! real per-item [`Tag`] (name, id, book count, average rating across
//! that item's books), computed via one join query per category
//! against the real schema (not the in-memory bulk-loaded `Table`
//! model upstream's `Field.get_categories` iterates -- same disclosed
//! simplification as everywhere else `Cache::field_for` already
//! deviates from that model). All three of upstream's real sort modes
//! (`name`/`popularity`/`rating`) are supported, matching upstream's
//! tie-breaking (`popularity`/`rating` break ties by name).
//!
//! Not included as a category here: `ratings` itself (upstream treats
//! it specially -- each "item" is a distinct star value, not a
//! link-table row with its own name/id -- and folding that into this
//! pass's uniform per-item-table loop isn't a natural fit; every
//! other category's own `avg_rating` field already surfaces rating
//! data usefully without it).
//!
//! `sort_key_for_name`'s real ICU-collation-based comparison (with
//! hierarchical-category dot-to-tab substitution) is approximated
//! here as a plain case-insensitive string comparison -- same
//! disclosed simplification `icu.rs`'s own docs already describe.

use crate::cache::Cache;
use anyhow::{bail, Result};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// A narrower version of upstream's `Tag` class: just the fields this
/// pass actually computes (name/id/count/average rating), not the
/// full set (`state`/`is_hierarchical`/`search_expression`/etc. -- all
/// tied to GUI selection state or features not ported here).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Tag {
    pub name: String,
    pub id: i32,
    pub count: usize,
    /// Average rating (0-5 stars) across the item's books, `0.0` if
    /// none of them have a nonzero rating.
    pub avg_rating: f64,
}

const STANDARD_CATEGORIES: &[(&str, &str, &str, &str, &str)] = &[
    // (category key, link table, link table's FK column, item table, item table's name column)
    ("authors", "books_authors_link", "author", "authors", "name"),
    ("tags", "books_tags_link", "tag", "tags", "name"),
    ("series", "books_series_link", "series", "series", "name"),
    (
        "publisher",
        "books_publishers_link",
        "publisher",
        "publishers",
        "name",
    ),
    (
        "languages",
        "books_languages_link",
        "lang_code",
        "languages",
        "lang_code",
    ),
];

fn category_from_link_table(
    conn: &rusqlite::Connection,
    link_table: &str,
    link_column: &str,
    item_table: &str,
    name_column: &str,
    book_ids: Option<&HashSet<i32>>,
    ratings: &HashMap<i32, i32>,
) -> Result<Vec<Tag>> {
    // `link_table`/`link_column`/`item_table`/`name_column` all come
    // from the fixed `STANDARD_CATEGORIES` list above, never user
    // input -- no injection risk from building this via `format!`,
    // same as this crate's other fixed-identifier SQL (e.g.
    // `custom_column_{id}` table names).
    let sql = format!(
        "SELECT {item_table}.id, {item_table}.{name_column}, {link_table}.book \
         FROM {link_table} JOIN {item_table} ON {item_table}.id = {link_table}.{link_column}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)?,
        ))
    })?;

    let mut items: IndexMap<i32, (String, HashSet<i32>)> = IndexMap::new();
    for row in rows {
        let (item_id, name, book_id) = row?;
        if let Some(filter) = book_ids {
            if !filter.contains(&book_id) {
                continue;
            }
        }
        items
            .entry(item_id)
            .or_insert_with(|| (name, HashSet::new()))
            .1
            .insert(book_id);
    }

    let mut tags = Vec::with_capacity(items.len());
    for (item_id, (name, book_id_set)) in items {
        let nonzero_ratings: Vec<i32> = book_id_set
            .iter()
            .filter_map(|b| ratings.get(b).copied())
            .filter(|&r| r > 0)
            .collect();
        let avg_rating = if nonzero_ratings.is_empty() {
            0.0
        } else {
            (nonzero_ratings.iter().sum::<i32>() as f64 / nonzero_ratings.len() as f64) / 2.0
        };
        tags.push(Tag {
            name,
            id: item_id,
            count: book_id_set.len(),
            avg_rating,
        });
    }
    Ok(tags)
}

fn sort_tags(tags: &mut [Tag], sort: &str) {
    match sort {
        "popularity" => tags.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        "rating" => tags.sort_by(|a, b| {
            b.avg_rating
                .partial_cmp(&a.avg_rating)
                .unwrap()
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        _ => tags.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }
}

/// Real per-category, per-item tag-browser data. `sort` must be one
/// of `"name"` (default upstream)/`"popularity"`/`"rating"`, matching
/// upstream's `CATEGORY_SORTS`; anything else is an error, same as
/// upstream's own `raise ValueError`. `book_ids` restricts the count
/// to a subset of books (e.g. a search result), matching upstream's
/// own optional filter.
pub fn get_categories(
    cache: &Arc<Mutex<Cache>>,
    sort: &str,
    book_ids: Option<&HashSet<i32>>,
) -> Result<IndexMap<String, Vec<Tag>>> {
    if !matches!(sort, "name" | "popularity" | "rating") {
        bail!("sort {sort} not a valid value");
    }

    let guard = cache.lock().unwrap();
    let conn = guard.backend.conn.lock().unwrap();

    let mut ratings: HashMap<i32, i32> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT books_ratings_link.book, ratings.rating \
             FROM books_ratings_link JOIN ratings ON ratings.id = books_ratings_link.rating",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?;
        for row in rows {
            let (book_id, rating) = row?;
            ratings.insert(book_id, rating);
        }
    }

    let mut categories = IndexMap::new();
    for &(key, link_table, link_column, item_table, name_column) in STANDARD_CATEGORIES {
        let mut tags = category_from_link_table(
            &conn,
            link_table,
            link_column,
            item_table,
            name_column,
            book_ids,
            &ratings,
        )?;
        sort_tags(&mut tags, sort);
        categories.insert(key.to_string(), tags);
    }

    Ok(categories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Arc<Mutex<Cache>>) {
        let dir = tempdir().unwrap();
        let backend = Backend::new(dir.path()).unwrap();
        (dir, Arc::new(Mutex::new(Cache::from_backend(backend))))
    }

    fn insert_book_with_author_tag_rating(
        cache: &Arc<Mutex<Cache>>,
        title: &str,
        author: &str,
        tag: &str,
        rating: Option<i32>,
    ) -> i32 {
        let guard = cache.lock().unwrap();
        let conn = guard.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES (?1)", [title])
            .unwrap();
        let book_id = conn.last_insert_rowid() as i32;

        conn.execute("INSERT OR IGNORE INTO authors (name) VALUES (?1)", [author])
            .unwrap();
        let author_id: i64 = conn
            .query_row("SELECT id FROM authors WHERE name = ?1", [author], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
            (book_id, author_id),
        )
        .unwrap();

        conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])
            .unwrap();
        let tag_id: i64 = conn
            .query_row("SELECT id FROM tags WHERE name = ?1", [tag], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO books_tags_link (book, tag) VALUES (?1, ?2)",
            (book_id, tag_id),
        )
        .unwrap();

        if let Some(rating) = rating {
            conn.execute(
                "INSERT OR IGNORE INTO ratings (rating) VALUES (?1)",
                [rating],
            )
            .unwrap();
            let rating_id: i64 = conn
                .query_row("SELECT id FROM ratings WHERE rating = ?1", [rating], |r| {
                    r.get(0)
                })
                .unwrap();
            conn.execute(
                "INSERT INTO books_ratings_link (book, rating) VALUES (?1, ?2)",
                (book_id, rating_id),
            )
            .unwrap();
        }

        book_id
    }

    #[test]
    fn get_categories_reports_real_counts_per_author() {
        let (_dir, cache) = open_test_cache();
        insert_book_with_author_tag_rating(&cache, "Book One", "Jane Doe", "fiction", None);
        insert_book_with_author_tag_rating(&cache, "Book Two", "Jane Doe", "drama", None);
        insert_book_with_author_tag_rating(&cache, "Book Three", "John Smith", "fiction", None);

        let cats = get_categories(&cache, "name", None).unwrap();
        let authors = &cats["authors"];
        assert_eq!(authors.len(), 2);
        let jane = authors.iter().find(|t| t.name == "Jane Doe").unwrap();
        assert_eq!(jane.count, 2);
        let tags = &cats["tags"];
        let fiction = tags.iter().find(|t| t.name == "fiction").unwrap();
        assert_eq!(fiction.count, 2);
    }

    #[test]
    fn get_categories_computes_average_rating_across_an_items_books() {
        let (_dir, cache) = open_test_cache();
        // Ratings are stored *2 internally (half-star granularity).
        insert_book_with_author_tag_rating(&cache, "A", "Jane Doe", "fiction", Some(8)); // 4 stars
        insert_book_with_author_tag_rating(&cache, "B", "Jane Doe", "fiction", Some(6)); // 3 stars
        insert_book_with_author_tag_rating(&cache, "C", "Jane Doe", "fiction", None); // unrated, excluded

        let cats = get_categories(&cache, "name", None).unwrap();
        let jane = cats["authors"]
            .iter()
            .find(|t| t.name == "Jane Doe")
            .unwrap();
        assert_eq!(jane.avg_rating, 3.5);
    }

    #[test]
    fn get_categories_sorts_by_popularity_descending_with_name_tiebreak() {
        let (_dir, cache) = open_test_cache();
        insert_book_with_author_tag_rating(&cache, "A", "Zed", "fiction", None);
        insert_book_with_author_tag_rating(&cache, "B", "Amy", "fiction", None);
        insert_book_with_author_tag_rating(&cache, "C", "Amy", "fiction", None);

        let cats = get_categories(&cache, "popularity", None).unwrap();
        let names: Vec<&str> = cats["authors"].iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Amy", "Zed"]);
    }

    #[test]
    fn get_categories_restricts_counts_to_the_given_book_ids() {
        let (_dir, cache) = open_test_cache();
        let id1 = insert_book_with_author_tag_rating(&cache, "A", "Jane Doe", "fiction", None);
        insert_book_with_author_tag_rating(&cache, "B", "Jane Doe", "fiction", None);

        let filter: HashSet<i32> = [id1].into_iter().collect();
        let cats = get_categories(&cache, "name", Some(&filter)).unwrap();
        let jane = cats["authors"]
            .iter()
            .find(|t| t.name == "Jane Doe")
            .unwrap();
        assert_eq!(jane.count, 1);
    }

    #[test]
    fn get_categories_rejects_an_invalid_sort_value() {
        let (_dir, cache) = open_test_cache();
        assert!(get_categories(&cache, "bogus", None).is_err());
    }
}
