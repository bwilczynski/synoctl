//! NAS domain logic, ported from homelab-api/internal/{docker,storage,system}.
//! One NAS per `Service`; IDs carry no device prefix.

mod backups;
pub mod docker;
mod info;
mod utilization;
mod volumes;

pub use info::parse_uptime;

use jiff::tz::TimeZone;

use crate::dsm::DsmApi;

pub struct Service {
    pub(crate) api: Box<dyn DsmApi>,
    /// The NAS's zone, for DSM timestamps that carry no offset.
    pub(crate) tz: TimeZone,
}

impl Service {
    pub fn new(api: Box<dyn DsmApi>, tz: TimeZone) -> Self {
        Self { api, tz }
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
