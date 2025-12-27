use calibre_utils::{constants, logging};
use log::info;

fn main() {
    logging::init();
    info!("Starting Calibre Oxide Demo...");
    
    println!("App Name: {}", constants::APP_NAME);
    println!("Version: {}", constants::VERSION);
    println!("Is Linux? {}", constants::is_linux());
    println!("Is Windows? {}", constants::is_windows());
    println!("Is MacOS? {}", constants::is_macos());
    println!("Cache Dir: {:?}", constants::cache_dir());
    println!("Config Dir: {:?}", constants::config_dir());
}
