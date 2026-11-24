use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum Event {
    MetadataChanged { field: String, book_ids: Vec<i32> },
    BookAdded { book_id: i32 },
    BookRemoved { book_ids: Vec<i32> },
    FormatAdded { book_id: i32, format: String },
    // Add others as needed
}

pub trait EventListener: Send + Sync {
    fn on_event(&self, event: &Event);
}

#[derive(Clone)]
pub struct EventDispatcher {
    listeners: Arc<Mutex<Vec<Box<dyn EventListener>>>>,
}

impl EventDispatcher {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn register(&self, listener: Box<dyn EventListener>) {
        let mut guard = self.listeners.lock().unwrap();
        guard.push(listener);
    }

    pub fn dispatch(&self, event: Event) {
        let guard = self.listeners.lock().unwrap();
        for listener in guard.iter() {
            listener.on_event(&event);
        }
    }
}
