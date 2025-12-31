use crate::backend::Backend;
use rusqlite::Result;
use std::path::Path;

pub struct Cache {
    pub backend: Backend,
}

impl Cache {
    pub fn new<P: AsRef<Path>>(library_path: P) -> Result<Self> {
        let mut backend = Backend::new(library_path)?;
        // In a real scenario, we might want to lazy load prefs or load them here
        backend.load_prefs().unwrap_or_else(|e| {
            eprintln!("Failed to load prefs: {}", e);
        });

        Ok(Cache { backend })
    }

    pub fn library_id(&self) -> String {
        // This is typically stored in prefs or generated
        self.backend
            .prefs
            .get("library_id")
            .cloned()
            .unwrap_or_default()
    }
    pub fn field_for(&self, book_id: i32, field_name: &str) -> Result<Option<String>> {
        self.backend.field_for(book_id, field_name)
    }

    pub fn update_memory(&mut self, _book_id: i32, _field: &str, _value: &str) {
        // Placeholder for future in-memory cache invalidation.
        // Currently, field_for hits the DB directly so no cache to clear.
    }
}
