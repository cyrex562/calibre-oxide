use anyhow::Result;
use std::path::Path;

pub struct LegacyDB;

impl LegacyDB {
    pub fn new() -> Self {
        LegacyDB
    }

    pub fn check_compatibility(&self, db_path: &Path) -> Result<bool> {
        // Placeholder: Checks if the database is an old legacy format that needs migration.
        // For now, assume it's NOT (false) or just return Ok(true) if we assume modern.
        // Let's assume we don't support legacy DBs, so return false if it looks legacy?
        // Actually, let's just implement a stub check.
        if !db_path.exists() {
            return Ok(true); // New DB is fine
        }

        // Real logic would check sqlite file header or specific table presence.
        // Returning true implies "Compatible w/ current code" (i.e., not legacy needing migration)
        Ok(true)
    }

    pub fn migrate(&self, _db_path: &Path) -> Result<()> {
        Err(anyhow::anyhow!(
            "Legacy database migration is not supported in this version."
        ))
    }
}
