//! Release sources read from image labels, so most images need no
//! `[[updates.sources]]` entry. DSM's image calls carry no labels, but
//! Docker copies them onto the container, so they come from
//! `SYNO.Docker.Container get`. A container's labels are fixed for its
//! lifetime, and pulling a new image or changing labels recreates it under
//! a new ID, so each container is read once and kept in `sources.json` by
//! ID.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::releases::Repo;

/// The OCI label naming the repository an image was built from.
pub(crate) const SOURCE_LABEL: &str = "org.opencontainers.image.source";

/// `https://github.com/owner/repo[.git][/…]` or the same on codeberg.org
/// → that repo; anything else → `None`.
pub(crate) fn label_repo(url: &str) -> Option<Repo> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let mut parts = path.splitn(3, '/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let path = format!("{owner}/{repo}");
    match host {
        "github.com" => Some(Repo::github(&path)),
        "codeberg.org" => Some(Repo::codeberg(&path)),
        _ => None,
    }
}

/// `sources.json`: what each container's source label says, by container
/// ID; `None` when it names no repo synoctl can query.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct SourceCache {
    #[serde(default)]
    containers: BTreeMap<String, Option<String>>,
}

impl SourceCache {
    /// An empty cache when `dir` is `None` or the file is missing or bad.
    pub(crate) fn read(dir: Option<&Path>) -> Self {
        dir.and_then(|d| std::fs::read_to_string(path(d)).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// `Some` when container `id` was read before.
    pub(crate) fn get(&self, id: &str) -> Option<Option<Repo>> {
        self.containers
            .get(id)
            .map(|k| k.as_deref().and_then(Repo::from_key))
    }

    pub(crate) fn insert(&mut self, id: &str, repo: Option<&Repo>) {
        self.containers.insert(id.to_string(), repo.map(Repo::key));
    }

    /// Keeps only containers in `ids`, so removed and recreated ones drop
    /// out; true when any did.
    pub(crate) fn retain(&mut self, ids: &[&str]) -> bool {
        let before = self.containers.len();
        self.containers.retain(|k, _| ids.contains(&k.as_str()));
        self.containers.len() != before
    }

    pub(crate) fn write(&self, dir: Option<&Path>) {
        let Some(dir) = dir else {
            return;
        };
        if let Ok(body) = serde_json::to_string(self) {
            let _ = crate::private_fs::create_dir(dir);
            let _ = crate::private_fs::replace(&path(dir), &body);
        }
    }
}

fn path(dir: &Path) -> PathBuf {
    dir.join("sources.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_forms() {
        assert_eq!(
            label_repo("https://github.com/dani-garcia/vaultwarden"),
            Some(Repo::github("dani-garcia/vaultwarden"))
        );
        assert_eq!(
            label_repo("https://github.com/grafana/grafana.git"),
            Some(Repo::github("grafana/grafana"))
        );
        assert_eq!(
            label_repo("https://github.com/a/b/tree/main/docker"),
            Some(Repo::github("a/b"))
        );
        assert_eq!(
            label_repo("https://codeberg.org/forgejo/forgejo"),
            Some(Repo::codeberg("forgejo/forgejo"))
        );
        assert_eq!(label_repo("https://gitlab.com/a/b"), None);
        assert_eq!(label_repo("https://github.com/solo"), None);
        assert_eq!(label_repo("github.com/a/b"), None);
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = SourceCache::default();
        c.insert("a1", Some(&Repo::github("grafana/grafana")));
        c.insert("b2", None);
        c.insert("gone", None);
        assert!(c.retain(&["a1", "b2"]));
        assert!(!c.retain(&["a1", "b2"]));
        c.write(Some(dir.path()));
        let c = SourceCache::read(Some(dir.path()));
        assert_eq!(c.get("a1"), Some(Some(Repo::github("grafana/grafana"))));
        assert_eq!(c.get("b2"), Some(None));
        assert_eq!(c.get("gone"), None);
    }
}
