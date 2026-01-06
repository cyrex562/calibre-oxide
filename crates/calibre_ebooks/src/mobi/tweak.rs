use anyhow::{bail, Result};
use std::path::Path;

pub fn explode(path: &Path, dest: &Path) -> Result<String> {
    // Stub implementation until MobiReader/Mobi8Reader are available.
    bail!("tweak::explode not implemented yet")
}

pub fn rebuild(src_dir: &Path, dest_path: &Path) -> Result<()> {
    // Stub implementation until Plumber/Input/Output plugins are available.
    bail!("tweak::rebuild not implemented yet")
}
