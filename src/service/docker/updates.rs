//! `docker updates`, from homelab-api/internal/system/updates_service.go:
//! running image tags against the latest upstream release.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;

use crate::config::SourceType;
use crate::error::{Error, Result};
use crate::model::docker::{Update, UpdateDetail, UpdateList, UpdateStatus};
use crate::releases::Repo;
use crate::service::{Service, Warned};

use super::sources::{SOURCE_LABEL, SourceCache, label_repo};

/// A tracked container before its release is known.
struct Candidate {
    name: String,
    image: String,
    tag: String,
    repo: Option<Repo>,
}

impl Service {
    /// Every container with a version tag; `status` keeps only those in that
    /// state, `force` ignores cached releases.
    pub fn list_updates(
        &self,
        status: Option<UpdateStatus>,
        force: bool,
        now: Timestamp,
    ) -> Result<Warned<UpdateList>> {
        let w = self.updates(force, now)?;
        let items = w
            .data
            .into_iter()
            .map(|d| d.update)
            .filter(|u| status.is_none_or(|s| u.status == s))
            .collect();
        Ok(Warned {
            data: UpdateList { items },
            warnings: w.warnings,
        })
    }

    pub fn get_update(&self, name: &str, now: Timestamp) -> Result<Warned<UpdateDetail>> {
        let w = self.updates(false, now)?;
        let data = w
            .data
            .into_iter()
            .find(|d| d.id == name)
            .ok_or_else(|| Error::NotFound(format!("Update not found: {name}")))?;
        Ok(Warned {
            data,
            warnings: w.warnings,
        })
    }

    fn updates(&self, force: bool, now: Timestamp) -> Result<Warned<Vec<UpdateDetail>>> {
        let containers = self.api.containers()?.containers;
        let mut warnings = Vec::new();
        let tracked: Vec<Tracked> = containers
            .iter()
            .filter_map(|c| {
                let (image, tag) = split_image_tag(&c.image);
                is_version_tag(tag).then_some(Tracked {
                    id: &c.id,
                    name: &c.name,
                    image,
                    tag,
                })
            })
            .collect();
        let labels = self.label_sources(&tracked, force, &mut warnings);
        let mut warned = BTreeSet::new();
        let candidates: Vec<Candidate> = tracked
            .into_iter()
            .map(|t| {
                let label = labels.get(&t.image_ref()).cloned().flatten();
                let repo = self.resolve(t.image, label);
                if repo.is_none() && warned.insert(t.image) {
                    let hint = if self.sources_hint.is_empty() {
                        "the config file"
                    } else {
                        self.sources_hint.as_str()
                    };
                    warnings.push(format!(
                        "no release source for {} (container {}); add it under [[updates.sources]] in {hint}",
                        t.image, t.name
                    ));
                }
                Candidate {
                    name: t.name.to_string(),
                    image: t.image.to_string(),
                    tag: t.tag.to_string(),
                    repo,
                }
            })
            .collect();
        let repos: BTreeSet<Repo> = candidates.iter().filter_map(|c| c.repo.clone()).collect();
        let (releases, release_warnings) = if repos.is_empty() {
            Default::default()
        } else {
            self.releases.latest(&repos, force)
        };
        warnings.extend(release_warnings);
        let data = candidates
            .into_iter()
            .map(|c| {
                let source = c
                    .repo
                    .as_ref()
                    .map_or_else(|| "https://github.com".to_string(), Repo::source_url);
                let release = c.repo.as_ref().and_then(|r| releases.get(r));
                let status = match (&c.repo, release) {
                    (None, _) => UpdateStatus::Unknown,
                    (Some(_), None) => UpdateStatus::Stale,
                    (Some(_), Some(r)) if normalize(&r.tag_name) == normalize(&c.tag) => {
                        UpdateStatus::UpToDate
                    }
                    (Some(_), Some(_)) => UpdateStatus::UpdateAvailable,
                };
                UpdateDetail {
                    update: Update {
                        id: c.name.clone(),
                        name: c.name,
                        kind: "container",
                        status,
                        latest_version: release
                            .map_or_else(|| c.tag.clone(), |r| r.tag_name.clone()),
                        current_version: c.tag,
                        checked_at: now,
                    },
                    image: c.image,
                    release_url: release
                        .map_or_else(|| format!("{source}/releases"), |r| r.html_url.clone()),
                    published_at: release.map_or(now, |r| r.published_at),
                    source,
                }
            })
            .collect();
        Ok(Warned { data, warnings })
    }

    /// `updates.sources` first, then the image's source label, then a
    /// `ghcr.io/owner/repo` image's own repo.
    fn resolve(&self, image: &str, label: Option<Repo>) -> Option<Repo> {
        self.configured(image)
            .or(label)
            .or_else(|| ghcr_repo(image).map(|p| Repo::new(SourceType::Github, &p)))
    }

    fn configured(&self, image: &str) -> Option<Repo> {
        self.sources
            .iter()
            .find(|s| s.image == image)
            .map(|s| Repo::new(s.r#type, &s.source))
    }

    /// The source label of each tracked image without an `updates.sources`
    /// entry, by `image:tag`: from `sources.json` when one of its containers
    /// was read before, else read from one of them, those reads in
    /// parallel. `force` reads every image again. Images whose read failed
    /// are absent.
    fn label_sources(
        &self,
        tracked: &[Tracked],
        force: bool,
        warnings: &mut Vec<String>,
    ) -> BTreeMap<String, Option<Repo>> {
        let dir = self.cache_dir.as_deref();
        let mut cache = if force {
            SourceCache::default()
        } else {
            SourceCache::read(dir)
        };
        let mut by_ref: BTreeMap<String, Vec<&Tracked>> = BTreeMap::new();
        for t in tracked
            .iter()
            .filter(|t| self.configured(t.image).is_none())
        {
            by_ref.entry(t.image_ref()).or_default().push(t);
        }
        let mut found = BTreeMap::new();
        let mut reads: Vec<(String, &Tracked)> = Vec::new();
        for (r, containers) in by_ref {
            match containers.iter().find_map(|t| cache.get(t.id)) {
                Some(repo) => {
                    found.insert(r, repo);
                }
                None => reads.push((r, containers[0])),
            }
        }
        let results = crate::parallel::map(&reads, |(_, t)| self.api.container(t.name));
        let read_any = !reads.is_empty();
        for ((r, t), result) in reads.into_iter().zip(results) {
            match result {
                Ok(d) => {
                    let label = d.details.config.labels.get(SOURCE_LABEL);
                    let repo = label.and_then(|u| label_repo(u));
                    cache.insert(t.id, repo.as_ref());
                    found.insert(r, repo);
                }
                // Removed since the list; its image is read next time.
                Err(Error::NotFound(_)) => {}
                Err(e) => warnings.push(format!("labels of container {}: {e}", t.name)),
            }
        }
        let ids: Vec<&str> = tracked.iter().map(|t| t.id).collect();
        if cache.retain(&ids) || read_any || force {
            cache.write(dir);
        }
        found
    }
}

/// A container with a version tag, as listed.
struct Tracked<'a> {
    id: &'a str,
    name: &'a str,
    image: &'a str,
    tag: &'a str,
}

impl Tracked<'_> {
    /// `image:tag`; containers running the same one share a source.
    fn image_ref(&self) -> String {
        format!("{}:{}", self.image, self.tag)
    }
}

/// `v1.2.3` and `1.2.3` are the same version.
fn normalize(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
}

/// `registry/image:tag` → (`registry/image`, `tag`); the tag is only looked
/// for after the last `/`, so a registry port is never read as a tag, and a
/// `@sha256:…` digest is dropped.
pub fn split_image_tag(image: &str) -> (&str, &str) {
    let image = image.split_once('@').map_or(image, |(name, _)| name);
    let name_start = image.rfind('/').map_or(0, |i| i + 1);
    match image[name_start..].rfind(':') {
        Some(i) => (&image[..name_start + i], &image[name_start + i + 1..]),
        None => (image, ""),
    }
}

/// Not empty, not `latest`, not a digest.
pub fn is_version_tag(tag: &str) -> bool {
    !(tag.is_empty() || tag == "latest" || tag.starts_with("sha256:"))
}

/// `ghcr.io/owner/repo[/…]` → `owner/repo`.
pub(crate) fn ghcr_repo(image: &str) -> Option<String> {
    let rest = image.strip_prefix("ghcr.io/")?;
    let mut parts = rest.splitn(3, '/');
    match (parts.next(), parts.next()) {
        (Some(owner), Some(repo)) if !owner.is_empty() && !repo.is_empty() => {
            Some(format!("{owner}/{repo}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::config::{SourceConfig, SourceType};
    use crate::model::Ids;
    use crate::releases::{FixtureReleases, Release, Repo};
    use crate::service::testutil::{fixtures, svc};

    fn now() -> Timestamp {
        "2026-04-25T12:00:00Z".parse().unwrap()
    }

    fn rel(repo: &str, tag: &str) -> (Repo, Release) {
        (
            Repo::github(repo),
            Release {
                tag_name: tag.into(),
                html_url: format!("https://github.com/{repo}/releases/tag/{tag}"),
                published_at: "2026-04-20T10:00:00Z".parse().unwrap(),
            },
        )
    }

    fn immich_releases() -> FixtureReleases {
        FixtureReleases::of([
            rel("immich-app/immich", "v2.6.3"),
            rel("immich-app/immich-server", "v2.6.3"),
            rel("immich-app/immich-machine-learning", "v2.7.0"),
        ])
    }

    fn prometheus_source() -> SourceConfig {
        SourceConfig {
            image: "prom/prometheus".into(),
            source: "prometheus/prometheus".into(),
            r#type: SourceType::Github,
        }
    }

    #[test]
    fn split_image_tag_forms() {
        assert_eq!(
            split_image_tag("ghcr.io/immich-app/immich-server:v2.6.3"),
            ("ghcr.io/immich-app/immich-server", "v2.6.3")
        );
        assert_eq!(split_image_tag("busybox"), ("busybox", ""));
        assert_eq!(
            split_image_tag("registry.lan:5000/app:1.2"),
            ("registry.lan:5000/app", "1.2")
        );
        assert_eq!(
            split_image_tag("registry.lan:5000/app"),
            ("registry.lan:5000/app", "")
        );
        assert_eq!(split_image_tag("app@sha256:abc"), ("app", ""));
        assert_eq!(split_image_tag("app:1.2@sha256:abc"), ("app", "1.2"));
    }

    #[test]
    fn version_tags() {
        for t in ["", "latest", "sha256:abc"] {
            assert!(!is_version_tag(t), "{t}");
        }
        assert!(is_version_tag("v2.6.3"));
    }

    #[test]
    fn ghcr_repo_forms() {
        assert_eq!(
            ghcr_repo("ghcr.io/immich-app/immich-server").as_deref(),
            Some("immich-app/immich-server")
        );
        assert_eq!(
            ghcr_repo("ghcr.io/owner/repo/sub").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(ghcr_repo("ghcr.io/solo"), None);
        assert_eq!(ghcr_repo("docker.io/a/b"), None);
    }

    #[test]
    fn ghcr_images_resolve_without_config() {
        let s =
            svc(fixtures()).with_updates(Box::new(immich_releases()), vec![], "cfg.toml".into());
        let w = s.list_updates(None, false, now()).unwrap();
        assert_eq!(
            w.data
                .items
                .iter()
                .map(|u| (u.id.as_str(), u.status))
                .collect::<Vec<_>>(),
            [
                ("immich_server", UpdateStatus::UpToDate),
                ("immich_machine_learning", UpdateStatus::UpdateAvailable),
                ("prometheus", UpdateStatus::Unknown)
            ]
        );
        let ml = &w.data.items[1];
        assert_eq!(
            (
                ml.current_version.as_str(),
                ml.latest_version.as_str(),
                ml.checked_at
            ),
            ("v2.6.3", "v2.7.0", now())
        );
        assert_eq!(
            w.warnings,
            [
                "no release source for prom/prometheus (container prometheus); add it under [[updates.sources]] in cfg.toml"
            ]
        );
    }

    /// Fixtures where `immich_server`'s image says it is built from `url`.
    fn labelled(url: &str) -> crate::dsm::fixtures::FixtureApi {
        let mut api = fixtures();
        api.details
            .get_mut("immich_server")
            .unwrap()
            .details
            .config
            .labels
            .insert(super::SOURCE_LABEL.into(), url.into());
        api
    }

    /// `immich-app/immich` is ahead of the `immich-server` repo the image
    /// name points at, so the status says which one was used.
    fn monorepo_releases() -> FixtureReleases {
        FixtureReleases::of([
            rel("immich-app/immich", "v2.7.0"),
            rel("immich-app/immich-server", "v2.6.3"),
        ])
    }

    #[test]
    fn source_label_wins_over_ghcr_name() {
        let s = svc(labelled("https://github.com/immich-app/immich")).with_updates(
            Box::new(monorepo_releases()),
            vec![],
            String::new(),
        );
        let d = s.get_update("immich_server", now()).unwrap().data;
        assert_eq!(d.source, "https://github.com/immich-app/immich");
        assert_eq!(d.update.status, UpdateStatus::UpdateAvailable);
    }

    #[test]
    fn configured_source_wins_over_label() {
        let src = SourceConfig {
            image: "ghcr.io/immich-app/immich-server".into(),
            source: "immich-app/immich-server".into(),
            r#type: SourceType::Github,
        };
        let s = svc(labelled("https://github.com/immich-app/immich")).with_updates(
            Box::new(monorepo_releases()),
            vec![src],
            String::new(),
        );
        let d = s.get_update("immich_server", now()).unwrap().data;
        assert_eq!(d.source, "https://github.com/immich-app/immich-server");
    }

    #[test]
    fn unsupported_label_host_falls_back_to_ghcr_name() {
        let s = svc(labelled("https://gitlab.com/immich-app/immich")).with_updates(
            Box::new(monorepo_releases()),
            vec![],
            String::new(),
        );
        let d = s.get_update("immich_server", now()).unwrap().data;
        assert_eq!(d.source, "https://github.com/immich-app/immich-server");
    }

    const IMMICH_SERVER_ID: &str =
        "09492d85a64c9bfb40fbf13ebd1ba45ca2bb148ac91e030fb52bb2529b892984";

    fn cached_service(api: crate::dsm::fixtures::FixtureApi, dir: &tempfile::TempDir) -> Service {
        svc(api)
            .with_updates(Box::new(monorepo_releases()), vec![], String::new())
            .with_cache_dir(dir.path().into())
    }

    #[test]
    fn source_labels_are_cached_per_container() {
        let dir = tempfile::tempdir().unwrap();
        cached_service(labelled("https://github.com/immich-app/immich"), &dir)
            .list_updates(None, false, now())
            .unwrap();
        // The container's details are gone, so only the cache can say.
        let mut api = fixtures();
        api.details.clear();
        let d = cached_service(api, &dir)
            .get_update("immich_server", now())
            .unwrap()
            .data;
        assert_eq!(d.source, "https://github.com/immich-app/immich");
    }

    #[test]
    fn a_recreated_container_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        cached_service(labelled("https://github.com/immich-app/immich"), &dir)
            .list_updates(None, false, now())
            .unwrap();
        let mut api = labelled("https://github.com/immich-app/immich-server");
        api.containers.as_mut().unwrap().containers[0].id = "recreated".into();
        let d = cached_service(api, &dir)
            .get_update("immich_server", now())
            .unwrap()
            .data;
        assert_eq!(d.source, "https://github.com/immich-app/immich-server");
    }

    #[test]
    fn force_reads_labels_again() {
        let dir = tempfile::tempdir().unwrap();
        cached_service(labelled("https://github.com/immich-app/immich"), &dir)
            .list_updates(None, false, now())
            .unwrap();
        let w = cached_service(
            labelled("https://github.com/immich-app/immich-server"),
            &dir,
        )
        .list_updates(None, true, now())
        .unwrap();
        let d = w
            .data
            .items
            .iter()
            .find(|u| u.id == "immich_server")
            .unwrap();
        assert_eq!(d.status, UpdateStatus::UpToDate);
    }

    #[test]
    fn removed_containers_drop_out_of_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut api = labelled("https://github.com/immich-app/immich");
        api.containers
            .as_mut()
            .unwrap()
            .containers
            .retain(|c| c.name == "immich_server");
        cached_service(api, &dir)
            .list_updates(None, false, now())
            .unwrap();
        let cached = || SourceCache::read(Some(dir.path())).get(IMMICH_SERVER_ID);
        assert!(cached().is_some());
        // Nothing to read on the second run, so only pruning writes.
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers.clear();
        cached_service(api, &dir)
            .list_updates(None, false, now())
            .unwrap();
        assert_eq!(cached(), None);
    }

    #[test]
    fn failed_label_read_warns_and_falls_back() {
        let mut api = labelled("https://github.com/immich-app/immich");
        api.fail_only = vec!["container"];
        let s = svc(api).with_updates(Box::new(monorepo_releases()), vec![], String::new());
        let w = s.get_update("immich_server", now()).unwrap();
        assert_eq!(w.data.source, "https://github.com/immich-app/immich-server");
        assert!(
            w.warnings
                .iter()
                .any(|m| m.starts_with("labels of container immich_server: ")),
            "{:?}",
            w.warnings
        );
    }

    #[test]
    fn configured_source_without_release_is_stale() {
        let s = svc(fixtures()).with_updates(
            Box::new(FixtureReleases::default()),
            vec![prometheus_source()],
            String::new(),
        );
        let p = s.get_update("prometheus", now()).unwrap().data;
        assert_eq!(
            (
                p.status,
                p.current_version.as_str(),
                p.latest_version.as_str()
            ),
            (UpdateStatus::Stale, "v3.2.1", "v3.2.1")
        );
        assert_eq!(p.image, "prom/prometheus");
        assert_eq!(p.source, "https://github.com/prometheus/prometheus");
        assert_eq!(
            p.release_url,
            "https://github.com/prometheus/prometheus/releases"
        );
        assert_eq!(p.published_at, now());
    }

    #[test]
    fn release_fills_url_and_date() {
        let s = svc(fixtures()).with_updates(Box::new(immich_releases()), vec![], String::new());
        let d = s.get_update("immich_machine_learning", now()).unwrap().data;
        assert_eq!(
            d.release_url,
            "https://github.com/immich-app/immich-machine-learning/releases/tag/v2.7.0"
        );
        assert_eq!(
            d.published_at,
            "2026-04-20T10:00:00Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(
            d.source,
            "https://github.com/immich-app/immich-machine-learning"
        );
    }

    #[test]
    fn configured_source_wins_over_ghcr() {
        let src = SourceConfig {
            image: "ghcr.io/immich-app/immich-machine-learning".into(),
            source: "immich-app/immich".into(),
            r#type: SourceType::Github,
        };
        let s = svc(fixtures()).with_updates(Box::new(immich_releases()), vec![src], String::new());
        let d = s.get_update("immich_machine_learning", now()).unwrap().data;
        assert_eq!(d.status, UpdateStatus::UpToDate);
    }

    #[test]
    fn v_prefix_mismatch_is_up_to_date() {
        let r = FixtureReleases::of([rel("prometheus/prometheus", "3.2.1")]);
        let s = svc(fixtures()).with_updates(Box::new(r), vec![prometheus_source()], String::new());
        assert_eq!(
            s.get_update("prometheus", now()).unwrap().data.status,
            UpdateStatus::UpToDate
        );
    }

    #[test]
    fn non_version_tags_are_not_tracked() {
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers[2].image = "prom/prometheus:latest".into();
        let s = svc(api).with_updates(Box::new(immich_releases()), vec![], String::new());
        assert_eq!(
            s.list_updates(None, false, now()).unwrap().data.ids(),
            ["immich_server", "immich_machine_learning"]
        );
    }

    #[test]
    fn status_filter() {
        let s = svc(fixtures()).with_updates(Box::new(immich_releases()), vec![], String::new());
        let l = s
            .list_updates(Some(UpdateStatus::UpdateAvailable), false, now())
            .unwrap();
        assert_eq!(l.data.ids(), ["immich_machine_learning"]);
    }

    #[test]
    fn get_unknown_is_not_found() {
        assert_eq!(
            svc(fixtures())
                .get_update("nope", now())
                .unwrap_err()
                .to_string(),
            "Update not found: nope"
        );
    }

    #[test]
    fn docker_missing_is_not_installed() {
        let mut api = fixtures();
        api.containers = None;
        assert!(matches!(
            svc(api).list_updates(None, false, now()),
            Err(crate::error::Error::NotInstalled(_))
        ));
    }

    #[test]
    fn json_shape() {
        let s = svc(fixtures()).with_updates(Box::new(immich_releases()), vec![], String::new());
        let v = serde_json::to_value(s.list_updates(None, false, now()).unwrap().data).unwrap();
        assert_eq!(v["items"][1]["status"], "updateAvailable");
        assert_eq!(v["items"][1]["type"], "container");
        assert_eq!(v["items"][1]["checkedAt"], "2026-04-25T12:00:00Z");
        assert!(v["items"][1].get("device").is_none());
    }
}
