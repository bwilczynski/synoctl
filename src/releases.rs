//! Latest releases on GitHub and Codeberg, for `docker updates`. Ported
//! from homelab-api/internal/system/github.go, with the server's in-memory
//! cache replaced by `releases.json` in the cache directory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::config::SourceType;
use crate::error::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(10);
/// Fetches at a time.
const PARALLEL: usize = 8;
const GITHUB_API: &str = "https://api.github.com";
const CODEBERG_API: &str = "https://codeberg.org/api/v1";

/// The fields synoctl reads from a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub published_at: Timestamp,
}

/// A repository on a release host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Repo {
    pub host: SourceType,
    /// `owner/repo`.
    pub path: String,
}

impl Repo {
    pub fn new(host: SourceType, path: &str) -> Self {
        Self {
            host,
            path: path.to_string(),
        }
    }

    pub fn github(path: &str) -> Self {
        Self::new(SourceType::Github, path)
    }

    pub fn codeberg(path: &str) -> Self {
        Self::new(SourceType::Codeberg, path)
    }

    /// The repository's web page.
    pub fn source_url(&self) -> String {
        match self.host {
            SourceType::Github => format!("https://github.com/{}", self.path),
            SourceType::Codeberg => format!("https://codeberg.org/{}", self.path),
        }
    }

    /// `github:owner/repo`, the key in `releases.json`.
    fn key(&self) -> String {
        let host = match self.host {
            SourceType::Github => "github",
            SourceType::Codeberg => "codeberg",
        };
        format!("{host}:{}", self.path)
    }

    fn from_key(key: &str) -> Option<Self> {
        let (host, path) = key.split_once(':')?;
        match host {
            "github" => Some(Self::github(path)),
            "codeberg" => Some(Self::codeberg(path)),
            _ => None,
        }
    }
}

/// Where `docker updates` gets releases.
pub trait Releases: Sync {
    /// The latest release of each repo; `force` ignores the cache. Repos
    /// whose lookup failed are absent unless an earlier one is cached; the
    /// warnings say why.
    fn latest(&self, repos: &BTreeSet<Repo>, force: bool)
    -> (BTreeMap<Repo, Release>, Vec<String>);
}

/// No release lookups at all.
pub struct NoReleases;

impl Releases for NoReleases {
    fn latest(&self, _: &BTreeSet<Repo>, _: bool) -> (BTreeMap<Repo, Release>, Vec<String>) {
        (BTreeMap::new(), Vec::new())
    }
}

pub struct HttpReleases {
    http: Client,
    cache_path: Option<PathBuf>,
    ttl: Duration,
    github_token: Option<String>,
    github_base: String,
    codeberg_base: String,
}

/// `releases.json`: the last lookups, and which repos they covered.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    fetched_at: Option<Timestamp>,
    /// Repos looked up at `fetched_at`, found or not.
    #[serde(default)]
    checked: BTreeSet<String>,
    #[serde(default)]
    releases: BTreeMap<String, Release>,
}

impl HttpReleases {
    /// `cache_dir` holds `releases.json`; lookups are reused for `ttl`.
    pub fn new(
        cache_dir: Option<&Path>,
        ttl: Duration,
        github_token: Option<String>,
    ) -> Result<Self> {
        let http = Client::builder()
            .user_agent("synoctl")
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| Error::Other(format!("release client: {e}")))?;
        Ok(Self {
            http,
            cache_path: cache_dir.map(|d| d.join("releases.json")),
            ttl,
            github_token: github_token.filter(|t| !t.is_empty()),
            github_base: GITHUB_API.into(),
            codeberg_base: CODEBERG_API.into(),
        })
    }

    #[cfg(test)]
    pub fn with_bases(mut self, github: String, codeberg: String) -> Self {
        self.github_base = github;
        self.codeberg_base = codeberg;
        self
    }

    fn read_cache(&self) -> CacheFile {
        self.cache_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write_cache(&self, cache: &CacheFile) {
        let Some(path) = &self.cache_path else {
            return;
        };
        if let (Some(dir), Ok(body)) = (path.parent(), serde_json::to_string(cache)) {
            let _ = crate::private_fs::create_dir(dir);
            let _ = crate::private_fs::replace(path, &body);
        }
    }

    fn fetch(&self, repo: &Repo) -> std::result::Result<Release, String> {
        let base = match repo.host {
            SourceType::Github => &self.github_base,
            SourceType::Codeberg => &self.codeberg_base,
        };
        let mut req = self
            .http
            .get(format!("{base}/repos/{}/releases/latest", repo.path))
            .header("accept", "application/json");
        if repo.host == SourceType::Github
            && let Some(token) = &self.github_token
        {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        match resp.status().as_u16() {
            200 => resp
                .json::<Release>()
                .map_err(|e| format!("unexpected response: {e}")),
            403 | 429 => Err("rate limited (set GITHUB_TOKEN)".into()),
            code => Err(format!("status {code}")),
        }
    }
}

impl Releases for HttpReleases {
    fn latest(
        &self,
        repos: &BTreeSet<Repo>,
        force: bool,
    ) -> (BTreeMap<Repo, Release>, Vec<String>) {
        let mut cache = self.read_cache();
        let fresh = cache.fetched_at.is_some_and(|t| {
            Timestamp::now()
                .duration_since(t)
                .try_into()
                .is_ok_and(|age: Duration| age < self.ttl)
        });
        let covered = repos.iter().all(|r| cache.checked.contains(&r.key()));
        let mut warnings = Vec::new();
        if force || !fresh || !covered {
            let list: Vec<&Repo> = repos.iter().collect();
            let results: Vec<(&Repo, std::result::Result<Release, String>)> = list
                .chunks(PARALLEL)
                .flat_map(|chunk| {
                    std::thread::scope(|s| {
                        let handles: Vec<_> = chunk
                            .iter()
                            .map(|r| (*r, s.spawn(|| self.fetch(r))))
                            .collect();
                        handles
                            .into_iter()
                            .map(|(r, h)| {
                                (r, h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            cache.fetched_at = Some(Timestamp::now());
            cache.checked = repos.iter().map(Repo::key).collect();
            for (repo, result) in results {
                match result {
                    Ok(release) => {
                        cache.releases.insert(repo.key(), release);
                    }
                    Err(e) => {
                        warnings.push(format!("release lookup for {} failed: {e}", repo.path))
                    }
                }
            }
            self.write_cache(&cache);
        }
        let found = cache
            .releases
            .into_iter()
            .filter_map(|(k, rel)| Repo::from_key(&k).map(|r| (r, rel)))
            .filter(|(r, _)| repos.contains(r))
            .collect();
        (found, warnings)
    }
}

/// Releases from fixture files, for tests and `--fixtures`.
#[cfg(any(test, feature = "fixtures"))]
#[derive(Debug, Clone, Default)]
pub struct FixtureReleases {
    pub releases: BTreeMap<Repo, Release>,
}

#[cfg(any(test, feature = "fixtures"))]
impl FixtureReleases {
    pub fn of(items: impl IntoIterator<Item = (Repo, Release)>) -> Self {
        Self {
            releases: items.into_iter().collect(),
        }
    }

    /// Every `github-release-*.json` in `dir` (GitHub's release JSON), by
    /// the repository its `html_url` names.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut releases = BTreeMap::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| Error::Other(format!("fixtures {}: {e}", dir.display())))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("github-release-") && name.ends_with(".json")) {
                continue;
            }
            let text = std::fs::read_to_string(entry.path())
                .map_err(|e| Error::Other(format!("fixture {name}: {e}")))?;
            let rel: Release = serde_json::from_str(&text)
                .map_err(|e| Error::Other(format!("fixture {name}: {e}")))?;
            let path: Vec<&str> = rel
                .html_url
                .trim_start_matches("https://github.com/")
                .splitn(3, '/')
                .collect();
            if let [owner, repo, _] = path.as_slice() {
                releases.insert(Repo::github(&format!("{owner}/{repo}")), rel);
            }
        }
        Ok(Self { releases })
    }
}

#[cfg(any(test, feature = "fixtures"))]
impl Releases for FixtureReleases {
    fn latest(&self, repos: &BTreeSet<Repo>, _: bool) -> (BTreeMap<Repo, Release>, Vec<String>) {
        let found = self
            .releases
            .iter()
            .filter(|(r, _)| repos.contains(r))
            .map(|(r, rel)| (r.clone(), rel.clone()))
            .collect();
        (found, Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use httpmock::prelude::*;
    use serde_json::json;

    use super::*;

    fn release_json(tag: &str, repo: &str) -> serde_json::Value {
        json!({"tag_name": tag, "html_url": format!("https://github.com/{repo}/releases/tag/{tag}"),
               "published_at": "2026-04-25T18:08:50Z", "name": tag, "draft": false})
    }

    fn client(server: &MockServer, dir: &tempfile::TempDir, token: Option<&str>) -> HttpReleases {
        HttpReleases::new(
            Some(dir.path()),
            Duration::from_secs(3600),
            token.map(String::from),
        )
        .unwrap()
        .with_bases(server.url("/gh"), server.url("/cb"))
    }

    fn repos(list: &[Repo]) -> BTreeSet<Repo> {
        list.iter().cloned().collect()
    }

    #[test]
    fn fetches_latest_from_github_and_codeberg() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let gh = server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest")
                .header("user-agent", "synoctl");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let cb = server.mock(|w, t| {
            w.path("/cb/repos/c/d/releases/latest");
            t.json_body(release_json("2.0", "c/d"));
        });
        let (got, warnings) = client(&server, &dir, None)
            .latest(&repos(&[Repo::github("a/b"), Repo::codeberg("c/d")]), false);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(got[&Repo::github("a/b")].tag_name, "v1.0");
        assert_eq!(
            got[&Repo::github("a/b")].published_at,
            "2026-04-25T18:08:50Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(got[&Repo::codeberg("c/d")].tag_name, "2.0");
        gh.assert_calls(1);
        cb.assert_calls(1);
    }

    #[test]
    fn sends_github_token_only_to_github() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let gh = server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest")
                .header("authorization", "Bearer T");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let cb = server.mock(|w, t| {
            w.path("/cb/repos/c/d/releases/latest")
                .header_missing("authorization");
            t.json_body(release_json("2.0", "c/d"));
        });
        let (got, _) = client(&server, &dir, Some("T"))
            .latest(&repos(&[Repo::github("a/b"), Repo::codeberg("c/d")]), false);
        assert_eq!(got.len(), 2);
        gh.assert_calls(1);
        cb.assert_calls(1);
    }

    #[test]
    fn fresh_cache_skips_http_and_force_ignores_it() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let gh = server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let want = repos(&[Repo::github("a/b")]);
        client(&server, &dir, None).latest(&want, false);
        let (got, _) = client(&server, &dir, None).latest(&want, false);
        assert_eq!(got[&Repo::github("a/b")].tag_name, "v1.0");
        gh.assert_calls(1);
        client(&server, &dir, None).latest(&want, true);
        gh.assert_calls(2);
    }

    #[test]
    fn a_repo_new_to_the_cache_is_fetched() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let cd = server.mock(|w, t| {
            w.path("/gh/repos/c/d/releases/latest");
            t.json_body(release_json("v2.0", "c/d"));
        });
        client(&server, &dir, None).latest(&repos(&[Repo::github("a/b")]), false);
        let (got, _) = client(&server, &dir, None)
            .latest(&repos(&[Repo::github("a/b"), Repo::github("c/d")]), false);
        assert_eq!(got.len(), 2);
        cd.assert_calls(1);
    }

    #[test]
    fn failed_fetch_keeps_previous_release() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let mut ok = server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let want = repos(&[Repo::github("a/b")]);
        client(&server, &dir, None).latest(&want, false);
        ok.delete();
        server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.status(500);
        });
        let (got, warnings) = client(&server, &dir, None).latest(&want, true);
        assert_eq!(got[&Repo::github("a/b")].tag_name, "v1.0");
        assert_eq!(warnings, ["release lookup for a/b failed: status 500"]);
    }

    #[test]
    fn rate_limit_without_cache_warns_and_omits() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let gh = server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.status(403);
        });
        let want = repos(&[Repo::github("a/b")]);
        let (got, warnings) = client(&server, &dir, None).latest(&want, false);
        assert!(got.is_empty());
        assert_eq!(
            warnings,
            ["release lookup for a/b failed: rate limited (set GITHUB_TOKEN)"]
        );
        // Tried within the interval: not asked again until it passes.
        let (_, warnings) = client(&server, &dir, None).latest(&want, false);
        assert!(warnings.is_empty());
        gh.assert_calls(1);
    }

    #[test]
    fn corrupt_cache_file_is_ignored() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("releases.json"), "x").unwrap();
        server.mock(|w, t| {
            w.path("/gh/repos/a/b/releases/latest");
            t.json_body(release_json("v1.0", "a/b"));
        });
        let (got, _) = client(&server, &dir, None).latest(&repos(&[Repo::github("a/b")]), false);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn repo_urls() {
        assert_eq!(Repo::github("a/b").source_url(), "https://github.com/a/b");
        assert_eq!(
            Repo::codeberg("c/d").source_url(),
            "https://codeberg.org/c/d"
        );
    }

    #[test]
    fn fixture_releases_load_by_html_url() {
        let r = FixtureReleases::load(&crate::dsm::fixtures::FixtureApi::default_dir()).unwrap();
        let (got, _) = r.latest(&repos(&[Repo::github("dani-garcia/vaultwarden")]), false);
        assert_eq!(
            got[&Repo::github("dani-garcia/vaultwarden")].tag_name,
            "1.35.8"
        );
    }
}
