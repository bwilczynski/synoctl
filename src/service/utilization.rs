//! `utilization`, from homelab-api/internal/system/utilization_service.go.

use jiff::Timestamp;

use super::Service;
use crate::error::Result;
use crate::model::{CpuUsage, DiskIo, MemoryUsage, NetworkInterfaceUsage, SystemUtilization};

/// DSM reports memory in KB.
const KB: i64 = 1024;

impl Service {
    pub fn utilization(&self, now: Timestamp) -> Result<SystemUtilization> {
        let u = self.api.system_utilization()?;
        let m = &u.memory;
        Ok(SystemUtilization {
            sampled_at: now,
            cpu: CpuUsage {
                user_percent: u.cpu.user_load,
                system_percent: u.cpu.system_load,
                total_percent: u.cpu.user_load + u.cpu.system_load + u.cpu.other_load,
            },
            memory: MemoryUsage {
                total_bytes: m.total_real * KB,
                available_bytes: m.avail_real * KB,
                used_percent: m.real_usage,
                swap_total_bytes: m.total_swap * KB,
                swap_used_bytes: (m.total_swap - m.avail_swap) * KB,
            },
            // `total` is the sum of the others.
            network: u
                .network
                .iter()
                .filter(|n| n.device != "total")
                .map(|n| NetworkInterfaceUsage {
                    name: n.device.clone(),
                    rx_bytes_per_sec: n.rx,
                    tx_bytes_per_sec: n.tx,
                })
                .collect(),
            disks: u
                .disk
                .disk
                .iter()
                .map(|d| DiskIo {
                    name: d.device.clone(),
                    read_ops_per_sec: d.read_access,
                    write_ops_per_sec: d.write_access,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CpuUsage, DiskIo, MemoryUsage, NetworkInterfaceUsage};
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn utilization_from_fixture() {
        let now: Timestamp = "2026-04-18T10:00:00Z".parse().unwrap();
        let u = svc(fixtures()).utilization(now).unwrap();
        assert_eq!(u.sampled_at, now);
        assert_eq!(
            u.cpu,
            CpuUsage {
                user_percent: 8,
                system_percent: 2,
                total_percent: 14
            }
        );
        assert_eq!(
            u.memory,
            MemoryUsage {
                total_bytes: 16234636 * 1024,
                available_bytes: 1616840 * 1024,
                used_percent: 46,
                swap_total_bytes: 11841452 * 1024,
                swap_used_bytes: (11841452 - 9618680) * 1024,
            }
        );
        assert_eq!(
            u.network,
            vec![NetworkInterfaceUsage {
                name: "bond0".into(),
                rx_bytes_per_sec: 10641,
                tx_bytes_per_sec: 5436
            }]
        );
        assert_eq!(u.disks.len(), 4);
        assert_eq!(
            u.disks[0],
            DiskIo {
                name: "sda".into(),
                read_ops_per_sec: 6,
                write_ops_per_sec: 30
            }
        );
    }

    #[test]
    fn json_uses_spec_names() {
        let now: Timestamp = "2026-04-18T10:00:00Z".parse().unwrap();
        let v = serde_json::to_value(svc(fixtures()).utilization(now).unwrap()).unwrap();
        assert_eq!(v["sampledAt"], "2026-04-18T10:00:00Z");
        assert_eq!(
            v["memory"]["swapUsedBytes"],
            (11841452_i64 - 9618680) * 1024
        );
        assert_eq!(v["network"][0]["rxBytesPerSec"], 10641);
        assert!(v.get("device").is_none());
    }
}
