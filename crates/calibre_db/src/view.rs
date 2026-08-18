use crate::cache::Cache;
use crate::search;
use std::sync::{Arc, Mutex};

pub struct View {
    cache: Arc<Mutex<Cache>>,
    ids: Vec<i32>,
}

impl View {
    /// A new view starts showing every book, matching upstream's
    /// initial (unrestricted, unsearched) view state.
    pub fn new(cache: Arc<Mutex<Cache>>) -> Self {
        let ids = cache.lock().unwrap().all_book_ids().unwrap_or_default();
        View { cache, ids }
    }

    pub fn search(&mut self, query: &str) {
        // Use our search module
        if let Ok(results) = search::search(&self.cache, query) {
            // In a real view, we might intersect with existing restriction.
            // For this basic port, simply replace ids with search results.
            self.ids = results;
        }
    }

    /// Sorts by the real value of `field` (via `Cache::field_for`), not
    /// upstream's `multisort` (which supports multiple fields with
    /// per-field direction, and uses ICU `sort_key` for text -- this
    /// does a single field, case-insensitive-lowercase text comparison
    /// like `Backend`'s `PYNOCASE` collation, same disclosed
    /// approximation as elsewhere in this crate). Numeric-looking
    /// values (`series_index`, `rating`, `size`, ...) sort
    /// numerically, not lexicographically -- `field_for` returns them
    /// as strings, so this parses first and falls back to string
    /// comparison for genuinely textual fields. Books with no value
    /// for `field` sort last regardless of direction.
    pub fn sort(&mut self, field: &str, ascending: bool) {
        let cache = self.cache.lock().unwrap();
        let keyed: Vec<(i32, Option<String>)> = self
            .ids
            .iter()
            .map(|&id| (id, cache.field_for(id, field).ok().flatten()))
            .collect();
        drop(cache);

        let (mut with_value, without_value): (Vec<_>, Vec<_>) =
            keyed.into_iter().partition(|(_, v)| v.is_some());

        with_value.sort_by(|(_, a), (_, b)| {
            let (a, b) = (a.as_ref().unwrap(), b.as_ref().unwrap());
            match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal),
                _ => a.to_lowercase().cmp(&b.to_lowercase()),
            }
        });
        if !ascending {
            with_value.reverse();
        }

        self.ids = with_value
            .into_iter()
            .chain(without_value)
            .map(|(id, _)| id)
            .collect();
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    pub fn get_ids(&self) -> &[i32] {
        &self.ids
    }
}
