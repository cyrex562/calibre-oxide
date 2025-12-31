use calibre_db::listeners::{Event, EventDispatcher, EventListener};
use std::sync::{Arc, Mutex};

struct MockListener {
    received_events: Arc<Mutex<Vec<String>>>,
}

impl EventListener for MockListener {
    fn on_event(&self, event: &Event) {
        let mut guard = self.received_events.lock().unwrap();
        match event {
            Event::BookAdded { book_id } => guard.push(format!("Added: {}", book_id)),
            _ => guard.push("Other".to_string()),
        }
    }
}

#[test]
fn test_event_dispatch() {
    let dispatcher = EventDispatcher::new();
    let events = Arc::new(Mutex::new(Vec::new()));

    let listener = MockListener {
        received_events: events.clone(),
    };
    dispatcher.register(Box::new(listener));

    dispatcher.dispatch(Event::BookAdded { book_id: 123 });

    let guard = events.lock().unwrap();
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0], "Added: 123");
}
