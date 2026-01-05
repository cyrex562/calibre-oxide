use crate::metadata::MetaInformation;
use anyhow::{bail, Result};
use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(_stream: R) -> Result<MetaInformation> {
    // RAR support in pure Rust is currently limited or requires C-bindings (e.g. compress-tools).
    // For now, this module is a placeholder.
    // In the future, we can integrate `compress-tools` or call out to `unrar` executable.
    bail!("RAR metadata extraction not yet supported in pure Rust")
}
