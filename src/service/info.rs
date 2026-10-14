//! `info`, from homelab-api/internal/system/info_service.go.

use super::Service;
use crate::error::Result;
use crate::model::SystemInfo;

impl Service {
    pub fn info(&self) -> Result<SystemInfo> {
        let info = self.api.system_info()?;
        Ok(SystemInfo {
            model: info.model,
            firmware: info.firmware_ver,
            ram_mb: info.ram_size,
            uptime_seconds: parse_uptime(&info.up_time).unwrap_or(0),
        })
    }
}

/// DSM's uptime, `H:M:S` with unbounded hours, in seconds.
pub fn parse_uptime(s: &str) -> Option<i64> {
    let parts: Vec<i64> = s
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [h, m, sec] => Some(h * 3600 + m * 60 + sec),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn info_from_fixture() {
        assert_eq!(
            svc(fixtures()).info().unwrap(),
            SystemInfo {
                model: "DS918+".into(),
                firmware: "DSM 7.1.1-42962 Update 9".into(),
                ram_mb: 16384,
                uptime_seconds: 360000,
            }
        );
    }

    #[test]
    fn parse_uptime_forms() {
        assert_eq!(parse_uptime("100:00:00"), Some(360000));
        assert_eq!(parse_uptime("1:02:03"), Some(3723));
        assert_eq!(parse_uptime("bad"), None);
        assert_eq!(parse_uptime("1:2"), None);
        assert_eq!(parse_uptime("1:x:3"), None);
    }

    #[test]
    fn unparseable_uptime_is_zero() {
        let mut api = fixtures();
        api.system_info.up_time = "x".into();
        assert_eq!(svc(api).info().unwrap().uptime_seconds, 0);
    }

    #[test]
    fn unreachable_nas_is_an_error() {
        let mut api = fixtures();
        api.fail = true;
        assert!(svc(api).info().is_err());
    }
}
