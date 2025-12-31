use crate::traits::{ConversionOptions, InputPlugin, OutputPlugin, Transform};
use anyhow::Result;
use std::path::Path;

pub struct ConversionPipeline {
    input: Box<dyn InputPlugin>,
    output: Box<dyn OutputPlugin>,
    transforms: Vec<Box<dyn Transform>>,
}

impl ConversionPipeline {
    pub fn new(input: Box<dyn InputPlugin>, output: Box<dyn OutputPlugin>) -> Self {
        Self {
            input,
            output,
            transforms: Vec::new(),
        }
    }

    pub fn add_transform(&mut self, transform: Box<dyn Transform>) {
        self.transforms.push(transform);
    }

    pub fn run(
        &self,
        input_path: &Path,
        output_path: &Path,
        options: &ConversionOptions,
    ) -> Result<()> {
        // 1. Read input to OEB
        let mut book = self.input.read(input_path, options)?;

        // 2. Process Transforms
        for transform in &self.transforms {
            transform.process(&mut book, options)?;
        }

        // 3. Write output
        self.output.write(&book, output_path, options)?;

        Ok(())
    }
}
