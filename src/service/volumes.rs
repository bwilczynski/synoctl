//! `volumes`, from homelab-api/internal/storage/volumes_service.go.

use super::Service;
use crate::dsm::types::{self, Storage};
use crate::error::{Error, Result};
use crate::model::{DiskStatus, Volume, VolumeDetail, VolumeDisk, VolumeList, VolumeStatus};

/// Query filters for `volumes list`; `None` means "do not filter".
#[derive(Debug, Clone, Default)]
pub struct VolumeFilter {
    pub status: Option<VolumeStatus>,
}

impl VolumeFilter {
    fn matches(&self, v: &Volume) -> bool {
        self.status.is_none_or(|s| v.status == s)
    }
}

impl Service {
    pub fn list_volumes(&self, filter: &VolumeFilter) -> Result<VolumeList> {
        let storage = self.api.storage()?;
        let mut items = volumes(&storage);
        items.retain(|v| filter.matches(v));
        Ok(VolumeList { items })
    }

    pub fn get_volume(&self, id: &str) -> Result<VolumeDetail> {
        let storage = self.api.storage()?;
        let raw = storage
            .volumes
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Error::NotFound(format!("Volume not found: {id}")))?;
        let pool = storage.storage_pools.iter().find(|p| p.id == raw.pool_path);
        let disks = pool.map_or_else(Vec::new, |pool| {
            pool.disks
                .iter()
                .filter_map(|d| storage.disks.iter().find(|disk| &disk.id == d))
                .map(disk)
                .collect()
        });
        Ok(VolumeDetail {
            volume: volume(raw),
            mount_path: raw.vol_path.clone(),
            pool_status: volume_status(pool.map_or("", |p| p.status.as_str())),
            disks,
        })
    }
}

pub(crate) fn volumes(storage: &Storage) -> Vec<Volume> {
    storage.volumes.iter().map(volume).collect()
}

fn volume(v: &types::Volume) -> Volume {
    Volume {
        id: v.id.clone(),
        name: v.id.clone(),
        status: volume_status(&v.status),
        raid_type: v.raid_type.clone(),
        file_system: v.fs_type.clone(),
        total_bytes: v.size.total.parse().unwrap_or(0),
        used_bytes: v.size.used.parse().unwrap_or(0),
    }
}

pub(crate) fn disk(d: &types::Disk) -> VolumeDisk {
    VolumeDisk {
        id: d.id.clone(),
        model: d.model.clone(),
        status: disk_status(&d.status),
        total_bytes: d.size_total.parse().unwrap_or(0),
        temperature_celsius: d.temp,
    }
}

/// Unknown states read as degraded, as in homelab-api.
pub(crate) fn volume_status(s: &str) -> VolumeStatus {
    match s {
        "normal" => VolumeStatus::Normal,
        "repairing" => VolumeStatus::Repairing,
        "crashed" => VolumeStatus::Crashed,
        _ => VolumeStatus::Degraded,
    }
}

/// Unknown states read as warning, as in homelab-api.
pub(crate) fn disk_status(s: &str) -> DiskStatus {
    match s {
        "normal" => DiskStatus::Normal,
        "failing" => DiskStatus::Failing,
        "critical" => DiskStatus::Critical,
        _ => DiskStatus::Warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Ids;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_from_fixture() {
        let l = svc(fixtures())
            .list_volumes(&VolumeFilter::default())
            .unwrap();
        assert_eq!(
            l.items,
            vec![Volume {
                id: "volume_1".into(),
                name: "volume_1".into(),
                status: VolumeStatus::Normal,
                raid_type: "single".into(),
                file_system: "btrfs".into(),
                total_bytes: 11508017246208,
                used_bytes: 8230588534784,
            }]
        );
        assert_eq!(l.ids(), ["volume_1"]);
    }

    #[test]
    fn list_filters_by_status() {
        let ids = |status| {
            svc(fixtures())
                .list_volumes(&VolumeFilter {
                    status: Some(status),
                })
                .unwrap()
                .ids()
        };
        assert_eq!(ids(VolumeStatus::Normal), ["volume_1"]);
        assert!(ids(VolumeStatus::Crashed).is_empty());
    }

    #[test]
    fn get_joins_pool_and_disks() {
        let d = svc(fixtures()).get_volume("volume_1").unwrap();
        assert_eq!(
            (d.mount_path.as_str(), d.pool_status),
            ("/volume1", VolumeStatus::Normal)
        );
        assert_eq!(
            d.disks
                .iter()
                .map(|d| (d.id.as_str(), d.temperature_celsius))
                .collect::<Vec<_>>(),
            [("sda", 35), ("sdb", 41), ("sdc", 41), ("sdd", 36)]
        );
        assert_eq!(d.disks[0].total_bytes, 4000787030016);
        assert_eq!(d.disks[0].status, DiskStatus::Normal);
        assert_eq!(d.disks[2].model, "Seagate IronWolf 4TB");
        assert_eq!(d.total_bytes, 11508017246208);
    }

    #[test]
    fn get_unknown_or_prefixed_is_not_found() {
        for id in ["volume_9", "nas-01.volume_1"] {
            assert_eq!(
                svc(fixtures()).get_volume(id).unwrap_err().to_string(),
                format!("Volume not found: {id}")
            );
        }
    }

    #[test]
    fn status_maps() {
        assert_eq!(volume_status("normal"), VolumeStatus::Normal);
        assert_eq!(volume_status("degraded"), VolumeStatus::Degraded);
        assert_eq!(volume_status("repairing"), VolumeStatus::Repairing);
        assert_eq!(volume_status("crashed"), VolumeStatus::Crashed);
        assert_eq!(volume_status("weird"), VolumeStatus::Degraded);
        assert_eq!(disk_status("normal"), DiskStatus::Normal);
        assert_eq!(disk_status("warning"), DiskStatus::Warning);
        assert_eq!(disk_status("failing"), DiskStatus::Failing);
        assert_eq!(disk_status("critical"), DiskStatus::Critical);
        assert_eq!(disk_status("weird"), DiskStatus::Warning);
    }

    #[test]
    fn unparseable_sizes_are_zero() {
        let mut api = fixtures();
        api.storage.volumes[0].size.total = String::new();
        let v = &svc(api)
            .list_volumes(&VolumeFilter::default())
            .unwrap()
            .items[0];
        assert_eq!(v.total_bytes, 0);
        assert_eq!(v.used_percent(), None);
    }

    #[test]
    fn used_percent() {
        let v = &svc(fixtures())
            .list_volumes(&VolumeFilter::default())
            .unwrap()
            .items[0];
        assert_eq!((v.used_percent().unwrap() * 10.0).round(), 715.0);
    }

    #[test]
    fn json_shape() {
        let v = serde_json::to_value(svc(fixtures()).get_volume("volume_1").unwrap()).unwrap();
        assert_eq!(v["poolStatus"], "normal");
        assert_eq!(v["fileSystem"], "btrfs");
        assert_eq!(v["disks"][0]["temperatureCelsius"], 35);
        assert!(v.get("device").is_none());
    }
}
