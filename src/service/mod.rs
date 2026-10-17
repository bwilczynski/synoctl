//! NAS domain logic, ported from homelab-api/internal/{docker,storage,system}.
//! One NAS per `Service`; IDs carry no device prefix.

mod backups;
pub mod docker;
mod info;
mod status;

pub use status::{DISK_WARN_C, VOLUME_ERROR_PCT, VOLUME_WARN_PCT};
mod utilization;
mod volumes;

pub use info::parse_uptime;

use std::path::PathBuf;

use jiff::tz::TimeZone;

use crate::config::SourceConfig;
use crate::dsm::DsmApi;
use crate::releases::{NoReleases, Releases};

pub struct Service {
    pub(crate) api: Box<dyn DsmApi>,
    /// The NAS's zone, for DSM timestamps that carry no offset.
    pub(crate) tz: TimeZone,
    pub(crate) releases: Box<dyn Releases>,
    /// `updates.sources` from the config.
    pub(crate) sources: Vec<SourceConfig>,
    /// Where `updates.sources` lives, for the "no release source" hint.
    pub(crate) sources_hint: String,
    /// Holds `sources.json`, the release sources read from image labels;
    /// `None` reads them every time.
    pub(crate) cache_dir: Option<PathBuf>,
}

impl Service {
    pub fn new(api: Box<dyn DsmApi>, tz: TimeZone) -> Self {
        Self {
            api,
            tz,
            releases: Box::new(NoReleases),
            sources: Vec::new(),
            sources_hint: String::new(),
            cache_dir: None,
        }
    }

    /// Release lookups for `docker updates` and `status`. `sources_hint`
    /// names the config file in the "no release source" warning.
    pub fn with_updates(
        mut self,
        releases: Box<dyn Releases>,
        sources: Vec<SourceConfig>,
        sources_hint: String,
    ) -> Self {
        self.releases = releases;
        self.sources = sources;
        self.sources_hint = sources_hint;
        self
    }

    /// Keeps the release sources read from image labels in `dir`.
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }
}

/// A result plus problems that did not stop it, for stderr.
#[derive(Debug, Clone, PartialEq)]
pub struct Warned<T> {
    pub data: T,
    pub warnings: Vec<String>,
}

#[cfg(test)]
pub(crate) mod testutil {
    use jiff::tz::TimeZone;

    use super::Service;
    use crate::dsm::fixtures::FixtureApi;

    pub(crate) fn fixtures() -> FixtureApi {
        FixtureApi::load(&FixtureApi::default_dir()).expect("load fixtures")
    }

    pub(crate) fn svc(api: FixtureApi) -> Service {
        Service::new(Box::new(api), TimeZone::UTC)
    }
}
