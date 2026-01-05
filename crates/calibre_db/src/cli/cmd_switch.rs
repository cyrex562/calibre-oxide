use anyhow::Result;
use calibre_utils::config::CONFIG;
use std::path::Path;

pub struct CmdSwitch;

impl CmdSwitch {
    pub fn new() -> Self {
        CmdSwitch
    }

    pub fn run(&self, args: &[String]) -> Result<()> {
        if args.is_empty() {
            return Err(anyhow::anyhow!("Usage: switch <library_path>"));
        }
        let library_path = Path::new(&args[0]);
        CONFIG.update_prefs(|prefs| {
            prefs.library_path = Some(library_path.to_string_lossy().to_string());
        })?;
        println!("Switched to library at {:?}", library_path);
        Ok(())
    }
}
