//! `status`: one screen answering "is anything wrong on the NAS?", in the
//! style of `unifictl status`. Never fails: an area that cannot be read is
//! `unknown`, and one whose DSM package is missing is left out.

use jiff::Timestamp;

use super::volumes::{disk, volumes};
use super::{BackupFilter, ContainerFilter, Service, UpdateFilter};
use crate::error::{Error, Result};
use crate::model::docker::{ContainerList, ContainerStatus, UpdateList, UpdateStatus};
use crate::model::{
    BackupTask, BackupTaskList, BackupTaskResult, BackupTaskStatus, DiskStatus, NasStatus,
    StatusArea, StatusRow, SystemInfo, Verdict, VolumeStatus,
};
use crate::render::format::{format_bytes, format_uptime};

/// Volume use that warns, and that is an error.
pub const VOLUME_WARN_PCT: f64 = 80.0;
pub const VOLUME_ERROR_PCT: f64 = 95.0;
/// Disk temperature that warns.
pub const DISK_WARN_C: i64 = 50;

impl Service {
    pub fn status(&self, now: Timestamp) -> NasStatus {
        let (info, storage, containers, backups, updates) = std::thread::scope(|s| {
            let info = s.spawn(|| self.api.system_info());
            let storage = s.spawn(|| self.api.storage());
            let containers = s.spawn(|| self.list_containers(&ContainerFilter::default()));
            let backups = s.spawn(|| self.list_backups(&BackupFilter::default()));
            let updates = s.spawn(|| self.list_updates(&UpdateFilter::default(), false, now));
            (
                join(info),
                join(storage),
                join(containers),
                join(backups),
                join(updates),
            )
        });
        let mut rows = vec![system_row(info.map(|i| {
            let uptime = super::parse_uptime(&i.up_time).unwrap_or(0);
            let warn = i.temperature_warning || i.sys_tempwarn;
            (
                SystemInfo {
                    model: i.model,
                    firmware: i.firmware_ver,
                    ram_mb: i.ram_size,
                    uptime_seconds: uptime,
                },
                i.sys_temp,
                warn,
            )
        }))];
        match storage {
            Ok(st) => {
                rows.push(volumes_row(&st));
                rows.push(disks_row(&st));
            }
            Err(e) => {
                rows.push(unknown(StatusArea::Volumes, "storage", &e));
                rows.push(unknown(StatusArea::Disks, "storage", &e));
            }
        }
        rows.extend(optional(
            StatusArea::Containers,
            "containers",
            containers,
            containers_row,
        ));
        rows.extend(optional(
            StatusArea::Backups,
            "backup tasks",
            backups.map(|w| w.data),
            backups_row,
        ));
        rows.extend(optional(StatusArea::Updates, "releases", updates, |w| {
            updates_row(&w.data, &w.warnings)
        }));
        NasStatus { rows }
    }
}

fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn row(area: StatusArea, verdict: Verdict, summary: String, problems: Vec<String>) -> StatusRow {
    StatusRow {
        area,
        verdict,
        summary,
        problems,
    }
}

fn unknown(area: StatusArea, what: &str, e: &Error) -> StatusRow {
    row(
        area,
        Verdict::Unknown,
        format!("cannot read {what}: {e}"),
        Vec::new(),
    )
}

/// A row for an area whose DSM package may be missing: left out then.
fn optional<T>(
    area: StatusArea,
    what: &str,
    r: Result<T>,
    f: impl FnOnce(T) -> StatusRow,
) -> Option<StatusRow> {
    match r {
        Ok(v) => Some(f(v)),
        Err(Error::NotInstalled(_)) => None,
        Err(e) => Some(unknown(area, what, &e)),
    }
}

fn worst(a: Verdict, b: Verdict) -> Verdict {
    let rank = |v| match v {
        Verdict::Ok | Verdict::Info => 0,
        Verdict::Unknown => 1,
        Verdict::Warn => 2,
        Verdict::Error => 3,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn system_row(r: Result<(SystemInfo, i64, bool)>) -> StatusRow {
    match r {
        Ok((info, temp, warn)) => row(
            StatusArea::System,
            if warn { Verdict::Warn } else { Verdict::Ok },
            format!(
                "{}  {}  up {}  {temp}°C",
                info.model,
                info.firmware,
                format_uptime(info.uptime_seconds)
            ),
            if warn {
                vec!["system temperature warning".into()]
            } else {
                Vec::new()
            },
        ),
        Err(e) => unknown(StatusArea::System, "system info", &e),
    }
}

fn volumes_row(st: &crate::dsm::types::Storage) -> StatusRow {
    let vols = volumes(st);
    let mut verdict = Verdict::Ok;
    let mut problems = Vec::new();
    for v in &vols {
        let state = match v.status {
            VolumeStatus::Normal => Verdict::Ok,
            VolumeStatus::Degraded | VolumeStatus::Repairing => Verdict::Warn,
            VolumeStatus::Crashed => Verdict::Error,
        };
        if state != Verdict::Ok {
            problems.push(format!("{} {}", v.id, v.status.as_str()));
            verdict = worst(verdict, state);
        }
        if let Some(pct) = v.used_percent().filter(|p| *p >= VOLUME_WARN_PCT) {
            problems.push(format!(
                "{} {pct:.0}% used ({} of {})",
                v.id,
                format_bytes(v.used_bytes),
                format_bytes(v.total_bytes)
            ));
            let level = if pct >= VOLUME_ERROR_PCT {
                Verdict::Error
            } else {
                Verdict::Warn
            };
            verdict = worst(verdict, level);
        }
    }
    row(
        StatusArea::Volumes,
        verdict,
        plural(vols.len(), "volume", "volumes"),
        problems,
    )
}

fn disks_row(st: &crate::dsm::types::Storage) -> StatusRow {
    let disks: Vec<_> = st.disks.iter().map(disk).collect();
    let mut verdict = Verdict::Ok;
    let mut problems = Vec::new();
    for d in &disks {
        let state = match d.status {
            DiskStatus::Normal => Verdict::Ok,
            DiskStatus::Warning => Verdict::Warn,
            DiskStatus::Failing | DiskStatus::Critical => Verdict::Error,
        };
        if state != Verdict::Ok {
            problems.push(format!("{} {}", d.id, d.status.as_str()));
            verdict = worst(verdict, state);
        }
        if d.temperature_celsius >= DISK_WARN_C {
            problems.push(format!("{} {}°C", d.id, d.temperature_celsius));
            verdict = worst(verdict, Verdict::Warn);
        }
    }
    let mut summary = plural(disks.len(), "disk", "disks");
    if let Some(max) = disks.iter().map(|d| d.temperature_celsius).max() {
        summary.push_str(&format!(", hottest {max}°C"));
    }
    row(StatusArea::Disks, verdict, summary, problems)
}

fn containers_row(list: ContainerList) -> StatusRow {
    let running = list
        .items
        .iter()
        .filter(|c| c.status == ContainerStatus::Running)
        .count();
    let mut verdict = Verdict::Ok;
    let mut problems = Vec::new();
    for (status, label, level) in [
        (ContainerStatus::Dead, "dead", Verdict::Error),
        (ContainerStatus::Restarting, "restarting", Verdict::Warn),
        (ContainerStatus::Paused, "paused", Verdict::Warn),
        (ContainerStatus::Stopped, "stopped", Verdict::Warn),
    ] {
        let names: Vec<&str> = list
            .items
            .iter()
            .filter(|c| c.status == status)
            .map(|c| c.name.as_str())
            .collect();
        if !names.is_empty() {
            problems.push(format!("{label}: {}", names.join(", ")));
            verdict = worst(verdict, level);
        }
    }
    row(
        StatusArea::Containers,
        verdict,
        format!("{running} of {} running", list.items.len()),
        problems,
    )
}

fn backups_row(list: BackupTaskList) -> StatusRow {
    let mut verdict = Verdict::Ok;
    let mut problems = Vec::new();
    type Matches = fn(&BackupTask) -> bool;
    let groups: [(&str, Verdict, Matches); 6] = [
        ("failed", Verdict::Error, |t| {
            t.last_result == BackupTaskResult::Failed
        }),
        ("error", Verdict::Error, |t| {
            t.status == BackupTaskStatus::Error
        }),
        ("warning", Verdict::Warn, |t| {
            t.last_result == BackupTaskResult::Warning
        }),
        ("skipped", Verdict::Warn, |t| {
            t.last_result == BackupTaskResult::Skipped
        }),
        ("suspended", Verdict::Warn, |t| {
            t.status == BackupTaskStatus::Suspended
        }),
        ("result unknown", Verdict::Warn, |t| {
            t.last_result == BackupTaskResult::Unknown
        }),
    ];
    for (label, level, matches) in groups {
        let names: Vec<&str> = list
            .items
            .iter()
            .filter(|t| matches(t))
            .map(|t| t.name.as_str())
            .collect();
        if !names.is_empty() {
            problems.push(format!("{label}: {}", names.join(", ")));
            verdict = worst(verdict, level);
        }
    }
    row(
        StatusArea::Backups,
        verdict,
        plural(list.items.len(), "task", "tasks"),
        problems,
    )
}

fn updates_row(list: &UpdateList, warnings: &[String]) -> StatusRow {
    let behind: Vec<String> = list
        .items
        .iter()
        .filter(|u| u.status == UpdateStatus::UpdateAvailable)
        .map(|u| format!("{} {} → {}", u.name, u.current_version, u.latest_version))
        .collect();
    if !behind.is_empty() {
        return row(
            StatusArea::Updates,
            Verdict::Info,
            format!("{} behind", plural(behind.len(), "container", "containers")),
            vec![behind.join(", ")],
        );
    }
    let lookup_failed = warnings
        .iter()
        .find(|w| w.starts_with("release lookup for "));
    let stale = list.items.iter().any(|u| u.status == UpdateStatus::Stale);
    if let (Some(w), true) = (lookup_failed, stale) {
        return row(
            StatusArea::Updates,
            Verdict::Unknown,
            format!("cannot read releases: {w}"),
            Vec::new(),
        );
    }
    let count = |st| list.items.iter().filter(|u| u.status == st).count();
    let mut parts = vec![format!(
        "{} of {} up to date",
        count(UpdateStatus::UpToDate),
        list.items.len()
    )];
    if stale {
        parts.push(format!("{} not checked yet", count(UpdateStatus::Stale)));
    }
    let unknown = count(UpdateStatus::Unknown);
    if unknown > 0 {
        parts.push(format!("{unknown} without a release source"));
    }
    let incomparable = count(UpdateStatus::Incomparable);
    if incomparable > 0 {
        parts.push(format!("{incomparable} with tags that cannot be compared"));
    }
    row(
        StatusArea::Updates,
        Verdict::Ok,
        parts.join(", "),
        Vec::new(),
    )
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::dsm::fixtures::FixtureApi;
    use crate::model::StatusArea::*;
    use crate::model::Verdict::*;
    use crate::releases::{FixtureReleases, Release, Repo};
    use crate::service::testutil::{fixtures, svc};

    fn now() -> Timestamp {
        "2026-04-25T12:00:00Z".parse().unwrap()
    }

    fn releases() -> FixtureReleases {
        FixtureReleases::of([
            (
                Repo::github("immich-app/immich-server"),
                Release {
                    tag_name: "v2.6.3".into(),
                    html_url: "https://github.com/immich-app/immich-server/releases/tag/v2.6.3"
                        .into(),
                    published_at: now(),
                },
            ),
            (
                Repo::github("immich-app/immich-machine-learning"),
                Release {
                    tag_name: "v2.7.0".into(),
                    html_url:
                        "https://github.com/immich-app/immich-machine-learning/releases/tag/v2.7.0"
                            .into(),
                    published_at: now(),
                },
            ),
        ])
    }

    fn status(api: FixtureApi) -> NasStatus {
        svc(api)
            .with_updates(Box::new(releases()), vec![], String::new())
            .status(now())
    }

    fn verdicts(s: &NasStatus) -> Vec<(StatusArea, Verdict)> {
        s.rows.iter().map(|r| (r.area, r.verdict)).collect()
    }

    fn row(s: &NasStatus, area: StatusArea) -> &StatusRow {
        s.rows.iter().find(|r| r.area == area).unwrap()
    }

    #[test]
    fn fixture_status() {
        let s = status(fixtures());
        assert_eq!(
            verdicts(&s),
            [
                (System, Ok),
                (Volumes, Ok),
                (Disks, Ok),
                (Containers, Warn),
                (Backups, Warn),
                (Updates, Info)
            ]
        );
        assert_eq!(
            row(&s, System).summary,
            "DS918+  DSM 7.1.1-42962 Update 9  up 4d 4h 0m 0s  43°C"
        );
        assert_eq!(row(&s, Volumes).summary, "1 volume");
        assert!(row(&s, Volumes).problems.is_empty());
        assert_eq!(row(&s, Disks).summary, "4 disks, hottest 41°C");
        assert_eq!(row(&s, Containers).summary, "2 of 3 running");
        assert_eq!(row(&s, Containers).problems, ["stopped: prometheus"]);
        assert_eq!(row(&s, Backups).summary, "2 tasks");
        assert_eq!(
            row(&s, Backups).problems,
            [
                "warning: Backup to LOCAL",
                "result unknown: Backup to REMOTE"
            ]
        );
        assert_eq!(row(&s, Updates).summary, "1 container behind");
        assert_eq!(
            row(&s, Updates).problems,
            ["immich_machine_learning v2.6.3 → v2.7.0"]
        );
        assert!(!s.has_error());
    }

    fn with_usage(api: &mut FixtureApi, pct: i64) {
        let v = &mut api.storage.volumes[0].size;
        v.total = "1000".into();
        v.used = (pct * 10).to_string();
    }

    #[test]
    fn volume_thresholds() {
        let mut api = fixtures();
        with_usage(&mut api, 79);
        assert_eq!(row(&status(api), Volumes).verdict, Ok);
        let mut api = fixtures();
        with_usage(&mut api, 80);
        let s = status(api);
        assert_eq!(row(&s, Volumes).verdict, Warn);
        assert_eq!(
            row(&s, Volumes).problems,
            ["volume_1 80% used (800 B of 1000 B)"]
        );
        let mut api = fixtures();
        with_usage(&mut api, 95);
        assert_eq!(row(&status(api), Volumes).verdict, Error);
        let mut api = fixtures();
        api.storage.volumes[0].status = "crashed".into();
        let s = status(api);
        assert_eq!(row(&s, Volumes).verdict, Error);
        assert_eq!(row(&s, Volumes).problems, ["volume_1 crashed"]);
        let mut api = fixtures();
        api.storage.volumes[0].status = "degraded".into();
        assert_eq!(row(&status(api), Volumes).verdict, Warn);
    }

    #[test]
    fn disk_thresholds() {
        let mut api = fixtures();
        api.storage.disks[1].temp = 50;
        let s = status(api);
        assert_eq!(row(&s, Disks).verdict, Warn);
        assert_eq!(row(&s, Disks).problems, ["sdb 50°C"]);
        for (st, v) in [("warning", Warn), ("failing", Error), ("critical", Error)] {
            let mut api = fixtures();
            api.storage.disks[0].status = st.into();
            let s = status(api);
            assert_eq!(row(&s, Disks).verdict, v, "{st}");
            assert_eq!(row(&s, Disks).problems, [format!("sda {st}")]);
        }
    }

    #[test]
    fn dead_container_is_error() {
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers[0].state.dead = true;
        let s = status(api);
        assert_eq!(row(&s, Containers).verdict, Error);
        assert_eq!(
            row(&s, Containers).problems,
            ["dead: immich_server", "stopped: prometheus"]
        );
        assert!(s.has_error());
    }

    #[test]
    fn all_running_is_ok() {
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers[2].state.running = true;
        assert_eq!(row(&status(api), Containers).verdict, Ok);
    }

    #[test]
    fn failed_backup_is_error() {
        let mut api = fixtures();
        api.backup_statuses.get_mut(&3).unwrap().last_bkp_result = "error".into();
        let s = status(api);
        assert_eq!(row(&s, Backups).verdict, Error);
        assert_eq!(row(&s, Backups).problems[0], "failed: Backup to LOCAL");
        assert!(s.has_error());
    }

    #[test]
    fn suspended_backup_warns() {
        let mut api = fixtures();
        api.backup_statuses.get_mut(&3).unwrap().last_bkp_error_code = 0;
        let mut tasks = api.backup_tasks.take().unwrap();
        tasks.task_list[0].state = "suspend".into();
        tasks.task_list.truncate(1);
        api.backup_tasks = Some(tasks);
        let s = status(api);
        assert_eq!(row(&s, Backups).verdict, Warn);
        assert_eq!(row(&s, Backups).problems, ["suspended: Backup to LOCAL"]);
    }

    #[test]
    fn healthy_backups_are_ok() {
        let mut api = fixtures();
        api.backup_statuses.get_mut(&3).unwrap().last_bkp_error_code = 0;
        api.backup_tasks.as_mut().unwrap().task_list.truncate(1);
        let s = status(api);
        assert_eq!(
            (row(&s, Backups).verdict, row(&s, Backups).summary.as_str()),
            (Ok, "1 task")
        );
    }

    #[test]
    fn system_temperature_warning() {
        let mut api = fixtures();
        api.system_info.temperature_warning = true;
        let s = status(api);
        assert_eq!(row(&s, System).verdict, Warn);
        assert_eq!(row(&s, System).problems, ["system temperature warning"]);
    }

    #[test]
    fn unreadable_area_is_unknown_not_failure() {
        let mut api = fixtures();
        api.fail_only = vec!["storage"];
        let s = status(api);
        assert_eq!(row(&s, Volumes).verdict, Unknown);
        assert_eq!(row(&s, Disks).verdict, Unknown);
        assert!(
            row(&s, Volumes)
                .summary
                .starts_with("cannot read storage: ")
        );
        assert_eq!(row(&s, System).verdict, Ok);
        assert!(!s.has_error());
    }

    #[test]
    fn missing_packages_drop_rows() {
        let mut api = fixtures();
        api.containers = None;
        api.backup_tasks = None;
        assert_eq!(
            verdicts(&status(api))
                .iter()
                .map(|(a, _)| *a)
                .collect::<Vec<_>>(),
            [System, Volumes, Disks]
        );
    }

    #[test]
    fn updates_up_to_date_or_unchecked() {
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers[1].image =
            "ghcr.io/immich-app/immich-machine-learning:v2.7.0".into();
        let s = status(api);
        assert_eq!(
            (row(&s, Updates).verdict, row(&s, Updates).summary.as_str()),
            (Ok, "2 of 3 up to date, 1 without a release source")
        );
        let s = svc(fixtures()).status(now());
        assert_eq!(
            (row(&s, Updates).verdict, row(&s, Updates).summary.as_str()),
            (
                Ok,
                "0 of 3 up to date, 2 not checked yet, 1 without a release source"
            )
        );
    }

    #[test]
    fn incomparable_tags_are_counted() {
        let mut api = fixtures();
        api.containers.as_mut().unwrap().containers[1].image =
            "ghcr.io/immich-app/immich-machine-learning:stable".into();
        let s = status(api);
        assert_eq!(
            (row(&s, Updates).verdict, row(&s, Updates).summary.as_str()),
            (
                Ok,
                "1 of 3 up to date, 1 without a release source, 1 with tags that cannot be compared"
            )
        );
    }

    struct RateLimited;
    impl crate::releases::Releases for RateLimited {
        fn latest(
            &self,
            repos: &std::collections::BTreeSet<Repo>,
            _: bool,
        ) -> (std::collections::BTreeMap<Repo, Release>, Vec<String>) {
            let warnings = repos
                .iter()
                .map(|r| {
                    format!(
                        "release lookup for {} failed: rate limited (set GITHUB_TOKEN)",
                        r.path
                    )
                })
                .collect();
            (Default::default(), warnings)
        }
    }

    #[test]
    fn failed_release_lookup_is_unknown() {
        let s = svc(fixtures())
            .with_updates(Box::new(RateLimited), vec![], String::new())
            .status(now());
        let r = row(&s, Updates);
        assert_eq!(r.verdict, Unknown);
        assert!(
            r.summary
                .starts_with("cannot read releases: release lookup for "),
            "{}",
            r.summary
        );
        assert!(!s.has_error());
    }

    #[test]
    fn json_is_rows() {
        let v = serde_json::to_value(status(fixtures())).unwrap();
        assert_eq!(v["rows"][3]["area"], "containers");
        assert_eq!(v["rows"][3]["verdict"], "warn");
        assert_eq!(v["rows"][3]["problems"][0], "stopped: prometheus");
    }
}
