//! Short-lived on-disk cache of raw DSM responses, so a `list`
//! followed by a few `get`s does not wait on the NAS every time.
//!
//! One file per request key under the cache directory. Entries are fresh for
//! `ttl` after they were written (file mtime). Every failure is a cache miss:
//! the cache only ever saves time, never fails a command.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
    /// False with `--refresh`: skip reads, but still store fresh responses.
    read: bool,
}

impl Cache {
    pub fn new(dir: PathBuf, ttl: Duration, read: bool) -> Self {
        Self { dir, ttl, read }
    }

    /// The body stored for `key` if it is younger than the TTL.
    pub fn get(&self, key: &str) -> Option<String> {
        self.get_within(key, self.ttl)
    }

    /// The body stored for `key` if it is younger than `max_age`, for
    /// responses that stay valid longer than the TTL.
    pub fn get_within(&self, key: &str, max_age: Duration) -> Option<String> {
        if !self.read {
            return None;
        }
        let path = self.path(key);
        let modified = fs::metadata(&path).ok()?.modified().ok()?;
        // A modification time in the future (clock change) is stale too.
        let age = SystemTime::now().duration_since(modified).ok()?;
        if age >= max_age {
            return None;
        }
        fs::read_to_string(path).ok()
    }

    /// Stores `body` for `key`, best effort. Written to a temporary file and
    /// renamed, so concurrent readers never see a partial body. Owner-only
    /// permissions: responses list every client on the network.
    pub fn put(&self, key: &str, body: &str) {
        let _ = self.try_put(key, body);
    }

    /// Drops the entry for `key`, best effort (after a write changes it).
    pub fn remove(&self, key: &str) {
        let _ = fs::remove_file(self.path(key));
    }

    fn try_put(&self, key: &str, body: &str) -> std::io::Result<()> {
        crate::private_fs::create_dir(&self.dir)?;
        crate::private_fs::replace(&self.path(key), body)
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", file_name(key)))
    }
}

/// Encodes `key` as a file name: `[A-Za-z0-9.-]` kept, every other byte
/// (including `_` itself) becomes `_XX`, so distinct keys never collide.
fn file_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("_{b:02x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn cache(dir: &tempfile::TempDir, ttl_secs: u64, read: bool) -> Cache {
        Cache::new(
            dir.path().join("synoctl"),
            Duration::from_secs(ttl_secs),
            read,
        )
    }

    #[test]
    fn round_trip_within_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 60, true);
        assert_eq!(c.get("https://h/a"), None);
        c.put("https://h/a", "{\"data\":[]}");
        assert_eq!(c.get("https://h/a").as_deref(), Some("{\"data\":[]}"));
        assert_eq!(c.get("https://h/b"), None);
    }

    #[test]
    fn expired_entry_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 60, true);
        c.put("k", "old");
        let f = fs::File::options().write(true).open(c.path("k")).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(61))
            .unwrap();
        assert_eq!(c.get("k"), None);
        c.put("k", "new");
        assert_eq!(c.get("k").as_deref(), Some("new"));
    }

    #[test]
    fn future_mtime_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 60, true);
        c.put("k", "v");
        let f = fs::File::options().write(true).open(c.path("k")).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
        assert_eq!(c.get("k"), None);
    }

    #[test]
    fn refresh_skips_reads_but_still_writes() {
        let dir = tempfile::tempdir().unwrap();
        let refresh = cache(&dir, 60, false);
        refresh.put("k", "v");
        assert_eq!(refresh.get("k"), None);
        assert_eq!(cache(&dir, 60, true).get("k").as_deref(), Some("v"));
    }

    #[test]
    fn unwritable_dir_is_silently_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        fs::write(&blocker, "").unwrap();
        let c = Cache::new(blocker.join("synoctl"), Duration::from_secs(60), true);
        c.put("k", "v");
        assert_eq!(c.get("k"), None);
    }

    #[test]
    fn file_names_are_safe_and_distinct() {
        assert_eq!(
            file_name("https://h:8443/a_b?x=1&y"),
            "https_3a_2f_2fh_3a8443_2fa_5fb_3fx_3d1_26y"
        );
        assert_ne!(file_name("a/b"), file_name("a_b"));
    }

    #[cfg(unix)]
    #[test]
    fn entries_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 60, true);
        c.put("k", "v");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.dir), 0o700);
        assert_eq!(mode(&c.path("k")), 0o600);
    }

    #[test]
    fn remove_drops_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 60, true);
        c.put("k", "v");
        c.remove("k");
        assert_eq!(c.get("k"), None);
        c.remove("missing");
    }

    #[test]
    fn get_within_uses_its_own_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let c = cache(&dir, 5, true);
        c.put("k", "v");
        let f = fs::File::options().write(true).open(c.path("k")).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        assert_eq!(c.get("k"), None);
        assert_eq!(
            c.get_within("k", Duration::from_secs(3600)).as_deref(),
            Some("v")
        );
        assert_eq!(c.get_within("k", Duration::from_secs(30)), None);
        assert_eq!(
            cache(&dir, 5, false).get_within("k", Duration::from_secs(3600)),
            None
        );
    }
}
