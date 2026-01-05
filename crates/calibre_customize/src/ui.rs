pub trait InterfaceAction {
    fn name(&self) -> &str;
    fn show_main_window(&self);
}

pub struct StubInterfaceAction {
    name: String,
}

impl StubInterfaceAction {
    pub fn new(name: &str) -> Self {
        StubInterfaceAction {
            name: name.to_string(),
        }
    }
}

impl InterfaceAction for StubInterfaceAction {
    fn name(&self) -> &str {
        &self.name
    }

    fn show_main_window(&self) {
        // Stub: Perform GUI action
        println!("Showing main window for action: {}", self.name);
    }
}
