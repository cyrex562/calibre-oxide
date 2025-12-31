use crate::oeb::OebBook;
use anyhow::Result;
use std::path::Path;

/// Options passed to the conversion process
pub struct ConversionOptions {
    pub input_profile: String,
    pub output_profile: String,
    // Add more options as needed
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            input_profile: "default".to_string(),
            output_profile: "default".to_string(),
        }
    }
}

/// Trait for reading an input format into the OEB intermediate representation
pub trait InputPlugin {
    fn read(&self, path: &Path, options: &ConversionOptions) -> Result<OebBook>;
}

/// Trait for writing the OEB intermediate representation to an output format
pub trait OutputPlugin {
    fn write(&self, book: &OebBook, path: &Path, options: &ConversionOptions) -> Result<()>;
}

/// Trait for transforming the OEB intermediate representation
pub trait Transform {
    fn process(&self, book: &mut OebBook, options: &ConversionOptions) -> Result<()>;
}
