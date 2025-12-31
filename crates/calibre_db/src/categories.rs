use crate::cache::Cache;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Retrieves aggregated category data for the side bar.
///
/// # Returns
/// * `Result<HashMap<String, Vec<String>>>` - A map where keys are category types (Authors, Series)
///   and values are lists of names.
pub fn get_categories(cache: &Arc<Mutex<Cache>>) -> Result<HashMap<String, Vec<String>>> {
    let guard = cache.lock().unwrap();
    let backend = &guard.backend;

    let mut categories = HashMap::new();

    // 1. Authors
    // We fetch all authors from the authors table.
    // In a real app, we might also want counts, sort order, etc.
    let authors_map = backend
        .get_all_authors()
        .context("Failed to fetch authors")?;
    // Convert to sorted vector of names
    let mut authors: Vec<String> = authors_map.into_values().collect();
    authors.sort_by_key(|a| a.to_lowercase());
    categories.insert("authors".to_string(), authors);

    // 2. Series
    let series_map = backend.get_all_series().context("Failed to fetch series")?;
    let mut series: Vec<String> = series_map.into_values().collect();
    series.sort_by_key(|s| s.to_lowercase());
    categories.insert("series".to_string(), series);

    // TODO: Tags, etc.

    Ok(categories)
}
