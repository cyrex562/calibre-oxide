pub mod oeb;
pub mod pipeline;
pub mod plugins;
pub mod traits;
pub mod transform; // New module

// Re-export key items
pub use oeb::OebBook;
pub use pipeline::ConversionPipeline;
pub use traits::{ConversionOptions, InputPlugin, OutputPlugin};
