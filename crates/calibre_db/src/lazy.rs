use crate::backend::Backend;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// ProxyMetadata mimics the lazy loading behavior of Calibre's Metadata object.
/// In Python, this heavily uses `__getattr__`. In Rust, we'll store a cache
/// and a reference to the backend to fetch missing fields.
pub struct ProxyMetadata {
    book_id: i32,
    backend: Arc<Mutex<Backend>>, // Using Arc<Mutex<>> for thread safety shared backend
    cache: HashMap<String, String>, // Simplified: storing everything as String for now. Real Calibre uses mixed types.
                                    // In reality, this should probably use an AnyMap or similar, or an enum for Value.
}

impl ProxyMetadata {
    pub fn new(book_id: i32, backend: Arc<Mutex<Backend>>) -> Self {
        Self {
            book_id,
            backend,
            cache: HashMap::new(),
        }
    }

    /// Simulates `simple_getter`.
    pub fn get_title(&mut self) -> String {
        if let Some(val) = self.cache.get("title") {
            return val.clone();
        }

        let backend = self.backend.lock().unwrap();
        match backend.field_for(self.book_id, "title") {
            Ok(Some(val)) => {
                self.cache.insert("title".to_string(), val.clone());
                val
            }
            Ok(None) | Err(_) => {
                let default = "Unknown".to_string();
                self.cache.insert("title".to_string(), default.clone());
                default
            }
        }
    }

    /// Simulates generic field access
    pub fn get_field(&mut self, field: &str) -> Option<String> {
        if let Some(val) = self.cache.get(field) {
            return Some(val.clone());
        }

        let backend = self.backend.lock().unwrap();
        match backend.field_for(self.book_id, field) {
            Ok(Some(val)) => {
                self.cache.insert(field.to_string(), val.clone());
                Some(val)
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("Error fetching field '{}': {}", field, e);
                None
            }
        }
    }
}

/// Placeholder for FormatMetadata
#[allow(dead_code)]
pub struct FormatMetadata {
    book_id: i32,
    formats: Vec<String>,
}

impl FormatMetadata {
    pub fn new(book_id: i32, formats: Vec<String>) -> Self {
        Self { book_id, formats }
    }
}
