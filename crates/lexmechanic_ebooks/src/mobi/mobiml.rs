use anyhow::{bail, Result};

pub struct MobiMLizer;

impl MobiMLizer {
    pub fn new() -> Self {
        MobiMLizer
    }

    pub fn process(&self) -> Result<()> {
        bail!("MobiMLizer not implemented yet")
    }
}
