use crate::cache::Cache;
use crate::search;
use std::sync::{Arc, Mutex};

pub struct View {
    cache: Arc<Mutex<Cache>>,
    ids: Vec<i32>,
}

impl View {
    pub fn new(cache: Arc<Mutex<Cache>>) -> Self {
        // Initially, the view should probably show all books.
        // We need a way to get all IDs.

        let mut ids = Vec::new();
        let conn_arc = {
            let lock = cache.lock().unwrap();
            lock.backend.conn.clone()
        };

        if let Ok(conn) = conn_arc.lock() {
            if let Ok(mut stmt) = conn.prepare("SELECT id FROM books") {
                if let Ok(rows) = stmt.query_map([], |row| row.get(0)) {
                    for r in rows {
                        if let Ok(id) = r {
                            ids.push(id);
                        }
                    }
                }
            }
        }

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

    pub fn sort(&mut self, _field: &str, _ascending: bool) {
        // Stub for sorting.
        // In reality, this would query field values and sort the self.ids vector.
        // For now, let's just sort by ID to ensure unstable sort doesn't mess things up randomly
        self.ids.sort();
        if !_ascending {
            self.ids.reverse();
        }

        // TODO: Implement actual field sorting using field_for
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    pub fn get_ids(&self) -> &[i32] {
        &self.ids
    }
}
