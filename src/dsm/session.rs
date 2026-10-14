//! The DSM session ID kept between runs, so each command does not log in
//! again (DSM records every login). Owner-only, and tied to the host and
//! account it was issued for.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Stored {
    host: String,
    username: String,
    sid: String,
}

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    /// `session.json` inside `dir` (the cache directory).
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("session.json"),
        }
    }

    /// The stored SID for this host and account; `None` when there is none,
    /// it belongs to another one, or the file cannot be read.
    pub fn load(&self, host: &str, username: &str) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let s: Stored = serde_json::from_str(&text).ok()?;
        (s.host == host && s.username == username).then_some(s.sid)
    }

    /// Best effort: a session that cannot be stored only costs a login.
    pub fn store(&self, host: &str, username: &str, sid: &str) {
        let body = serde_json::json!({"host": host, "username": username, "sid": sid}).to_string();
        if let Some(dir) = self.path.parent() {
            let _ = crate::private_fs::create_dir(dir);
        }
        let _ = crate::private_fs::replace(&self.path, &body);
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_for_the_same_host_and_user() {
        let dir = tempfile::tempdir().unwrap();
        let s = SessionStore::new(dir.path());
        assert_eq!(s.load("nas", "u"), None);
        s.store("nas", "u", "S1");
        assert_eq!(s.load("nas", "u").as_deref(), Some("S1"));
        assert_eq!(s.load("other", "u"), None);
        assert_eq!(s.load("nas", "admin"), None);
        s.clear();
        assert_eq!(s.load("nas", "u"), None);
    }

    #[test]
    fn corrupt_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("session.json"), "garbage").unwrap();
        assert_eq!(SessionStore::new(dir.path()).load("nas", "u"), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let s = SessionStore::new(&dir.path().join("synoctl"));
        s.store("nas", "u", "S1");
        let mode = std::fs::metadata(dir.path().join("synoctl/session.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
