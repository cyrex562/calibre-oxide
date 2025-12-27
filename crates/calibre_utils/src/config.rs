use log::info;

pub struct Config {
    pub library_path: Option<String>,
}

impl Config {
    pub fn new() -> Self {
        Config {
            library_path: None,
        }
    }

    pub fn load() -> Self {
        info!("Loading configuration...");
        // valid implementation pending
        Self::new()
    }
}
