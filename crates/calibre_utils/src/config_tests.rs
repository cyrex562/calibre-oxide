#[cfg(test)]
mod tests {
    use crate::config::{Config, GlobalPrefs};
    use crate::constants::config_dir;
    use std::fs;
    use std::path::PathBuf;

    // Use a unique temp dir for tests to avoid overlapping with real config or other tests
    fn setup_test_config() -> (Config, PathBuf) {
        let test_uuid = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("calibre_test_config_{}", test_uuid));
        fs::create_dir_all(&temp_dir).unwrap();

        // We can't easily mock Config::new() because it hardcodes constants::config_dir().
        // However, for unit testing default values, we can just test GlobalPrefs directly.
        // For testing file I/O, we might need to refactor Config to accept a path, or just assume
        // we are testing the logic assuming internal path construction is correct.
        
        // Actually, since Config::new() uses lazy_static and hardcodes path, integration testing it is tricky 
        // without environment variable overrides or changes to the code.
        // BUT, GlobalPrefs serialization and logic can be tested in isolation.

        let config = Config::new(); // This might try to write to actual config dir if unrelated logic runs, 
                                    // but we want to test load/save logic.
        
        // Let's rely on testing GlobalPrefs directly for now.
        (config, temp_dir)
    }

    #[test]
    fn test_global_prefs_defaults() {
        let prefs = GlobalPrefs::default();
        assert_eq!(prefs.network_timeout, 5);
        assert_eq!(prefs.output_format, "EPUB");
        assert_eq!(prefs.read_file_metadata, true);
        assert_eq!(prefs.worker_process_priority, "normal");
        assert_eq!(prefs.manage_device_metadata, "manual");
        assert_eq!(prefs.filename_pattern, "(?P<title>.+) - (?P<author>[^_]+)");
    }

    #[test]
    fn test_global_prefs_serialization() {
        let prefs = GlobalPrefs::default();
        let json = serde_json::to_string(&prefs).expect("Failed to serialize");
        let deserialized: GlobalPrefs = serde_json::from_str(&json).expect("Failed to deserialize");
        
        assert_eq!(prefs.network_timeout, deserialized.network_timeout);
        assert_eq!(prefs.filename_pattern, deserialized.filename_pattern);
        assert_eq!(prefs.input_format_order, deserialized.input_format_order);
    }
}
