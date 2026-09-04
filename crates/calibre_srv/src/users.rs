//! Port of `calibre.srv.users`'s `UserManager` -- the server's user
//! credential store.
//!
//! # `rusqlite`, not `apsw`
//!
//! Upstream uses `apsw` (a different Python SQLite binding) for this
//! one file; every other SQLite access anywhere in this workspace goes
//! through `rusqlite`, so this port does too. Same schema, same SQL.
//!
//! # Plaintext password storage -- intentional, matches upstream
//!
//! Passwords are stored as plaintext in the `users` table, matching
//! upstream exactly. This looks alarming out of context, but it's a
//! deliberate, disclosed upstream design decision (see the comment this
//! port carries over onto [`UserManager::new`]'s schema-creation code):
//! HTTP Digest authentication (calibre's preferred default auth mode --
//! see `opts.py`'s `auth_mode`) requires the server to reconstruct the
//! client's response hash, which needs either the plaintext password or
//! an unsalted hash of it (equally brute-forceable, so no real security
//! win). calibre's own threat model here is a private/LAN server, with
//! "run it behind an HTTPS reverse proxy" as the documented mitigation
//! for anyone who cares about network sniffing. This port preserves
//! that tradeoff rather than silently changing the security properties
//! of a upstream-compatible schema, and preserves upstream's own code
//! comment explaining why.
//!
//! # Not yet ported
//!
//! `restriction`/`misc_data` (per-user library-access restrictions and
//! the `allow_change_password_via_http` flag) are kept as schema
//! columns (for forward/backward compatibility with upstream's own
//! `userdb` files) but not exposed through any accessor yet -- nothing
//! in this server enforces per-library restrictions yet. `session_data`
//! (issue #500, part of the #432 browser-UI epic) IS now real --
//! [`UserManager::get_session_data`]/[`UserManager::set_session_data`],
//! backing `calibre_srv::ajax`'s `GET`/`POST /ajax/session-data`. `user_data`
//! (the bulk get/set-all-users property) also isn't ported; `add_user`/
//! `remove_user`/single-user accessors cover everything
//! `manage_users_cli.py`-equivalent tooling would need for now.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

/// Port of `validate_username`.
pub fn validate_username(username: &str) -> Option<String> {
    if username.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ')) {
        return Some(
            "For maximum compatibility you should use only the letters A-Z, the numbers 0-9, spaces, underscores and hyphens in the username"
                .to_string(),
        );
    }
    None
}

/// Port of `validate_password`.
pub fn validate_password(pw: &str) -> Option<String> {
    if pw.is_empty() {
        return Some("Empty passwords are not allowed".to_string());
    }
    if !pw.is_ascii() {
        return Some("The password must contain only ASCII (English) characters and symbols".to_string());
    }
    None
}

/// Port of `UserManager`.
pub struct UserManager {
    conn: Mutex<Connection>,
}

impl UserManager {
    /// Port of `UserManager.__init__` + the lazy `conn` property's
    /// schema-creation code (done eagerly here instead of on first
    /// access -- no behavioral difference, `rusqlite::Connection` is
    /// already a real connection object with nothing to lazily defer).
    pub fn new(path: &Path) -> anyhow::Result<UserManager> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if user_version == 0 {
            // We have to store the unhashed password, since the digest
            // auth scheme requires it -- see this module's doc.
            conn.execute_batch(
                r#"
                CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    pw TEXT NOT NULL,
                    timestamp TEXT DEFAULT CURRENT_TIMESTAMP,
                    session_data TEXT NOT NULL DEFAULT "{}",
                    restriction TEXT NOT NULL DEFAULT "{}",
                    readonly TEXT NOT NULL DEFAULT "n",
                    misc_data TEXT NOT NULL DEFAULT "{}",
                    UNIQUE(name)
                );
                PRAGMA user_version=1;
                "#,
            )?;
        }
        Ok(UserManager { conn: Mutex::new(conn) })
    }

    /// Convenience constructor matching upstream's own default path
    /// (`config_dir/server-users.sqlite`) -- `config_dir` isn't ported
    /// anywhere in this workspace (see other modules' own notes on that
    /// gap), so the caller supplies it explicitly.
    pub fn with_default_path(config_dir: &Path) -> anyhow::Result<UserManager> {
        Self::new(&config_dir.join("server-users.sqlite"))
    }

    /// Port of `get`: the password for `username`, or `None` if the
    /// user doesn't exist.
    pub fn get(&self, username: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT pw FROM users WHERE name=?1", [username], |row| row.get(0)).ok()
    }

    /// Port of `has_user`.
    pub fn has_user(&self, username: &str) -> bool {
        self.get(username).is_some()
    }

    /// Port of `get_session_data`: the user's stored client-prefs
    /// blob, or an empty object if the user doesn't exist or has never
    /// saved anything (matches upstream's own "no rows -> `{}`, not an
    /// error" behavior -- `session_data`'s own column default is
    /// already `"{}"`, so an existing user with no real data also
    /// falls out of this the same way).
    pub fn get_session_data(&self, username: &str) -> serde_json::Value {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn.query_row("SELECT session_data FROM users WHERE name=?1", [username], |row| row.get(0)).ok();
        raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| serde_json::json!({}))
    }

    /// Port of `set_session_data`: overwrites the whole stored blob.
    /// Upstream's own endpoint (`interface-data/set-session-data`)
    /// merges `new_data` into the existing dict before calling this --
    /// that merge is the caller's job here too (matching upstream's own
    /// split between the endpoint doing the merge and this method just
    /// doing the write), see `calibre_srv::ajax`'s own session-data
    /// handler. A `username` that doesn't exist is silently a no-op
    /// (`UPDATE` affecting zero rows is not an error), matching
    /// upstream's own anonymous-user no-op (`if rd.username: ...`).
    pub fn set_session_data(&self, username: &str, data: &serde_json::Value) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE users SET session_data=?1 WHERE name=?2", (data.to_string(), username))?;
        Ok(())
    }

    fn validate_username_for_add(&self, username: &str) -> Option<String> {
        if self.has_user(username) {
            return Some(format!("The username {username} already exists"));
        }
        validate_username(username)
    }

    /// Port of `add_user`.
    pub fn add_user(&self, username: &str, pw: &str, readonly: bool) -> Result<(), String> {
        if let Some(msg) = self.validate_username_for_add(username).or_else(|| validate_password(pw)) {
            return Err(msg);
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (name, pw, restriction, readonly, misc_data) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![username, pw, "{}", if readonly { "y" } else { "n" }, r#"{"allow_change_password_via_http": true}"#],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Port of `remove_user`: `true` if a user was actually removed.
    pub fn remove_user(&self, username: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM users WHERE name=?1", [username]).map(|n| n > 0).unwrap_or(false)
    }

    /// Port of the `all_user_names` property.
    pub fn all_user_names(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM users").unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.filter_map(Result::ok).collect()
    }

    /// Port of `is_readonly`.
    pub fn is_readonly(&self, username: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT readonly FROM users WHERE name=?1", [username], |row| row.get::<_, String>(0))
            .map(|v| v == "y")
            .unwrap_or(false)
    }

    /// Port of `set_readonly`.
    pub fn set_readonly(&self, username: &str, value: bool) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("UPDATE users SET readonly=?1 WHERE name=?2", rusqlite::params![if value { "y" } else { "n" }, username]);
    }

    /// Port of `change_password`.
    pub fn change_password(&self, username: &str, pw: &str) -> Result<(), String> {
        if let Some(msg) = validate_password(pw) {
            return Err(msg);
        }
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE users SET pw=?1 WHERE name=?2", rusqlite::params![pw, username]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("No such user: {username}"));
        }
        Ok(())
    }
}

/// Upstream's own default (`config_dir/server-users.sqlite`), exposed
/// so callers that already have a `config_dir` don't need to hand-build
/// the path themselves. See [`UserManager::with_default_path`].
pub fn default_userdb_path(config_dir: &Path) -> PathBuf {
    config_dir.join("server-users.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> (tempfile::TempDir, UserManager) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = UserManager::new(&dir.path().join("users.sqlite")).unwrap();
        (dir, mgr)
    }

    #[test]
    fn add_get_and_remove_a_user() {
        let (_dir, mgr) = test_manager();
        assert!(!mgr.has_user("alice"));
        mgr.add_user("alice", "hunter2", false).unwrap();
        assert!(mgr.has_user("alice"));
        assert_eq!(mgr.get("alice"), Some("hunter2".to_string()));
        assert_eq!(mgr.all_user_names(), vec!["alice".to_string()]);
        assert!(mgr.remove_user("alice"));
        assert!(!mgr.has_user("alice"));
        assert!(!mgr.remove_user("alice"));
    }

    #[test]
    fn session_data_defaults_to_an_empty_object() {
        let (_dir, mgr) = test_manager();
        mgr.add_user("alice", "hunter2", false).unwrap();
        assert_eq!(mgr.get_session_data("alice"), serde_json::json!({}));
    }

    #[test]
    fn session_data_round_trips_through_set_and_get() {
        let (_dir, mgr) = test_manager();
        mgr.add_user("alice", "hunter2", false).unwrap();
        mgr.set_session_data("alice", &serde_json::json!({"font_size": 16, "theme": "dark"})).unwrap();
        assert_eq!(mgr.get_session_data("alice"), serde_json::json!({"font_size": 16, "theme": "dark"}));
    }

    #[test]
    fn set_session_data_for_an_unknown_user_is_a_silent_no_op() {
        let (_dir, mgr) = test_manager();
        // Matches upstream's own anonymous-user no-op -- an UPDATE
        // affecting zero rows is not an error here either.
        mgr.set_session_data("nobody", &serde_json::json!({"x": 1})).unwrap();
        assert_eq!(mgr.get_session_data("nobody"), serde_json::json!({}));
    }

    #[test]
    fn get_session_data_for_an_unknown_user_is_an_empty_object() {
        let (_dir, mgr) = test_manager();
        assert_eq!(mgr.get_session_data("nobody"), serde_json::json!({}));
    }

    #[test]
    fn add_user_rejects_a_duplicate_username() {
        let (_dir, mgr) = test_manager();
        mgr.add_user("alice", "hunter2", false).unwrap();
        let err = mgr.add_user("alice", "different", false).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn add_user_rejects_an_empty_password() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.add_user("alice", "", false).is_err());
    }

    #[test]
    fn add_user_rejects_a_non_ascii_password() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.add_user("alice", "h\u{00fc}nter2", false).is_err());
    }

    #[test]
    fn validate_username_rejects_disallowed_characters() {
        assert!(validate_username("valid_name-123").is_none());
        assert!(validate_username("bad/name").is_some());
    }

    #[test]
    fn readonly_flag_round_trips() {
        let (_dir, mgr) = test_manager();
        mgr.add_user("alice", "hunter2", false).unwrap();
        assert!(!mgr.is_readonly("alice"));
        mgr.set_readonly("alice", true);
        assert!(mgr.is_readonly("alice"));
    }

    #[test]
    fn change_password_updates_the_stored_password() {
        let (_dir, mgr) = test_manager();
        mgr.add_user("alice", "hunter2", false).unwrap();
        mgr.change_password("alice", "new-password").unwrap();
        assert_eq!(mgr.get("alice"), Some("new-password".to_string()));
    }

    #[test]
    fn change_password_errors_for_an_unknown_user() {
        let (_dir, mgr) = test_manager();
        assert!(mgr.change_password("nobody", "hunter2").is_err());
    }

    #[test]
    fn reopening_the_same_database_preserves_users() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.sqlite");
        {
            let mgr = UserManager::new(&path).unwrap();
            mgr.add_user("alice", "hunter2", false).unwrap();
        }
        let mgr2 = UserManager::new(&path).unwrap();
        assert_eq!(mgr2.get("alice"), Some("hunter2".to_string()));
    }
}
