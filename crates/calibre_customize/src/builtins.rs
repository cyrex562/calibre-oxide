use anyhow::Result;

pub struct BuiltinPlugins;

impl BuiltinPlugins {
    pub fn new() -> Self {
        BuiltinPlugins
    }

    pub fn list_plugins(&self) -> Vec<String> {
        // Stub: Return a list of supported builtin plugin names
        vec![
            "MOBI Output".to_string(),
            "EPUB Output".to_string(),
            "PDF Output".to_string(),
        ]
    }
}
