//! Release sources read from image labels, so most images need no
//! `[[updates.sources]]` entry. DSM's image calls carry no labels, but
//! Docker copies them onto the container, so they come from
//! `SYNO.Docker.Container get`. An image's labels only change with its tag,
//! so each `image:tag` is looked up once and kept in `sources.json`.

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

/// `sources.json`: what each `image:tag`'s label says, `None` when it names
/// no repo synoctl can query.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct SourceCache {
    #[serde(default)]
    images: BTreeMap<String, Option<String>>,
}

impl SourceCache {
    /// An empty cache when `dir` is `None` or the file is missing or bad.
    pub(crate) fn read(dir: Option<&Path>) -> Self {
        dir.and_then(|d| std::fs::read_to_string(path(d)).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// `Some` when `image_ref` was looked up before.
    pub(crate) fn get(&self, image_ref: &str) -> Option<Option<Repo>> {
        self.images
            .get(image_ref)
            .map(|k| k.as_deref().and_then(Repo::from_key))
    }

    pub(crate) fn insert(&mut self, image_ref: String, repo: Option<&Repo>) {
        self.images.insert(image_ref, repo.map(Repo::key));
    }

    /// Keeps only `image_refs`, so pulled-over tags drop out.
    pub(crate) fn retain(&mut self, image_refs: &[String]) {
        self.images.retain(|k, _| image_refs.contains(k));
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
        c.insert(
            "grafana/grafana:13.0.1".into(),
            Some(&Repo::github("grafana/grafana")),
        );
        c.insert("grafana/loki:3.7.1".into(), None);
        c.insert("old/image:1".into(), None);
        c.retain(&["grafana/grafana:13.0.1".into(), "grafana/loki:3.7.1".into()]);
        c.write(Some(dir.path()));
        let c = SourceCache::read(Some(dir.path()));
        assert_eq!(
            c.get("grafana/grafana:13.0.1"),
            Some(Some(Repo::github("grafana/grafana")))
        );
        assert_eq!(c.get("grafana/loki:3.7.1"), Some(None));
        assert_eq!(c.get("old/image:1"), None);
    }
}
