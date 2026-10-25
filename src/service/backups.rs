//! `backups`, from homelab-api/internal/storage/backups_service.go.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

use super::{Service, Warned};
use crate::dsm::types;
use crate::error::{Error, Result};
use crate::model::{
    BackupTask, BackupTaskDetail, BackupTaskList, BackupTaskResult, BackupTaskStatus,
};

/// Query filters for `backups list`; `None` means "do not filter".
#[derive(Debug, Clone, Default)]
pub struct BackupFilter {
    pub status: Option<BackupTaskStatus>,
    pub result: Option<BackupTaskResult>,
}

impl BackupFilter {
    fn matches(&self, t: &BackupTask) -> bool {
        self.status.is_none_or(|s| t.status == s) && self.result.is_none_or(|r| t.last_result == r)
    }
}

impl Service {
    /// Every task with its last result, as far as `filter` keeps it. A task
    /// whose status cannot be read shows `unknown`, with a warning.
    pub fn list_backups(&self, filter: &BackupFilter) -> Result<Warned<BackupTaskList>> {
        let tasks = self.api.backup_tasks()?.task_list;
        let statuses: Vec<Result<types::BackupTaskStatus>> = std::thread::scope(|s| {
            let handles: Vec<_> = tasks
                .iter()
                .map(|t| s.spawn(|| self.api.backup_status(t.task_id)))
                .collect();
            handles.into_iter().map(join).collect()
        });
        let mut warnings = Vec::new();
        let items = tasks
            .iter()
            .zip(statuses)
            .map(|(t, status)| {
                let status = ok_or_warn(status, &t.name, "status", &mut warnings);
                task(t, status.as_ref())
            })
            .filter(|t| filter.matches(t))
            .collect();
        Ok(Warned {
            data: BackupTaskList { items },
            warnings,
        })
    }

    /// One task with its times, size and folders. Lookups that fail leave
    /// their fields out, with a warning.
    pub fn get_backup(&self, id: &str) -> Result<Warned<BackupTaskDetail>> {
        let tasks = self.api.backup_tasks()?.task_list;
        let t = id
            .parse::<i64>()
            .ok()
            .and_then(|n| tasks.iter().find(|t| t.task_id == n))
            .ok_or_else(|| Error::NotFound(format!("Backup task not found: {id}")))?;
        let mut warnings = Vec::new();
        let data = self.detail(t, &mut warnings);
        Ok(Warned { data, warnings })
    }

    /// Every task in detail, for `backups list --wide`.
    pub fn list_backups_wide(
        &self,
        filter: &BackupFilter,
    ) -> Result<Warned<Vec<BackupTaskDetail>>> {
        let tasks = self.api.backup_tasks()?.task_list;
        let results: Vec<Warned<BackupTaskDetail>> = std::thread::scope(|s| {
            let handles: Vec<_> = tasks
                .iter()
                .map(|t| {
                    s.spawn(|| {
                        let mut warnings = Vec::new();
                        let data = self.detail(t, &mut warnings);
                        Warned { data, warnings }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                .collect()
        });
        let mut warnings = Vec::new();
        let data = results
            .into_iter()
            .map(|w| {
                warnings.extend(w.warnings);
                w.data
            })
            .filter(|d| filter.matches(&d.task))
            .collect();
        Ok(Warned { data, warnings })
    }

    fn detail(&self, t: &types::BackupTask, warnings: &mut Vec<String>) -> BackupTaskDetail {
        let id = t.task_id;
        let (status, detail, target) = std::thread::scope(|s| {
            let status = s.spawn(|| self.api.backup_status(id));
            let detail = s.spawn(|| self.api.backup_task(id));
            let target = s.spawn(|| self.api.backup_target(id));
            (join(status), join(detail), join(target))
        });
        let status = ok_or_warn(status, &t.name, "status", warnings);
        let detail = ok_or_warn(detail, &t.name, "detail", warnings);
        let target = ok_or_warn(target, &t.name, "target", warnings);
        let folders: Vec<String> = detail
            .iter()
            .flat_map(|d| &d.source.folder_list)
            .filter(|f| !f.full_path.is_empty())
            .map(|f| f.full_path.clone())
            .collect();
        BackupTaskDetail {
            task: task(t, status.as_ref()),
            last_run_at: status
                .as_ref()
                .and_then(|s| parse_backup_time(&s.last_bkp_success_time, &self.tz)),
            next_run_at: status
                .as_ref()
                .and_then(|s| parse_backup_time(&s.next_bkp_time, &self.tz)),
            size: target.map(|t| t.used_size),
            folders: (!folders.is_empty()).then_some(folders),
        }
    }
}

fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn ok_or_warn<T>(r: Result<T>, name: &str, what: &str, warnings: &mut Vec<String>) -> Option<T> {
    r.map_err(|e| warnings.push(format!("backup task {name}: {what} unavailable: {e}")))
        .ok()
}

pub(crate) fn task(t: &types::BackupTask, status: Option<&types::BackupTaskStatus>) -> BackupTask {
    let last = status.map(|s| s.last_bkp_result.as_str());
    BackupTask {
        id: t.task_id.to_string(),
        name: t.name.clone(),
        kind: backup_type(&t.kind),
        status: backup_status(&t.state, &t.status, last),
        last_result: backup_result(status),
    }
}

/// DSM's `YYYY/MM/DD HH:MM`, in the NAS's zone.
pub(crate) fn parse_backup_time(s: &str, tz: &TimeZone) -> Option<Timestamp> {
    let dt = DateTime::strptime("%Y/%m/%d %H:%M", s).ok()?;
    dt.to_zoned(tz.clone()).ok().map(|z| z.timestamp())
}

// The two mappings below follow Hyper Backup's own UI (3.0.x, `util.js`:
// `getStatusIcon`, `getStatusText`, `GetBkpResultStr`), which labels a task
// from its `state`, then its `status`, then `last_bkp_result`. DSM ignores
// `last_bkp_error_code` there: 4401 is a generic internal error that
// successful tasks carry too.

/// The task's last run, as DSM labels it once the task is idle.
pub(crate) fn backup_result(status: Option<&types::BackupTaskStatus>) -> BackupTaskResult {
    let Some(s) = status else {
        return BackupTaskResult::Unknown;
    };
    match s.last_bkp_result.as_str() {
        // "No backup yet" when the settings changed since.
        "done" | "success" | "partial" if s.is_modified => BackupTaskResult::Unknown,
        "done" | "success" => BackupTaskResult::Success,
        "partial" => BackupTaskResult::Warning,
        "cancel" => BackupTaskResult::Canceled,
        // "No backup yet", "Waiting to check server status", and a suspended
        // run (the task's status says so) have no outcome yet. An absent
        // field is not DSM's to label.
        "none" | "failed_checking" | "backingup" | "resuming" | "suspend" | "" => {
            BackupTaskResult::Unknown
        }
        // `failed`, `discard`, `version_delete_failed`, and anything else:
        // DSM shows a failure.
        _ => BackupTaskResult::Failed,
    }
}

/// What the task is doing, from the task list's `state` and `status`; `last`
/// is `last_bkp_result`, which marks a suspended task.
pub(crate) fn backup_status(state: &str, status: &str, last: Option<&str>) -> BackupTaskStatus {
    use BackupTaskStatus::*;
    let idle = || {
        if last == Some("suspend") {
            Suspended
        } else {
            Idle
        }
    };
    match state {
        // DSM treats a task without a state as backupable.
        "backupable" | "" => match status {
            "backup"
            | "waiting"
            | "suspending"
            | "discarding"
            | "canceling"
            | "wait_discard"
            | "wait_version_delete"
            | "version_deleting"
            | "preparing_version_delete" => Running,
            "discard_failed" => Error,
            _ => idle(),
        },
        "exportable" => match status {
            "backup" | "waiting" | "suspending" | "discarding" | "canceling" => Running,
            "discard_failed" => Error,
            _ => idle(),
        },
        "importable" => match status {
            "none" => Idle,
            "canceling" => Error,
            _ => Running,
        },
        "relinkable" => match status {
            "relink" | "relink_waiting" | "backup" | "waiting" => Running,
            _ => Error,
        },
        "error_detect" => Checking,
        "broken" if status == "deleting" => Running,
        "restore_only" => Idle,
        // `unauth`, `endofservice`, `broken`, and states DSM does not know.
        _ => Error,
    }
}

pub(crate) fn backup_type(t: &str) -> String {
    if t.starts_with("image:") {
        "hyperBackup".into()
    } else if t.contains("glacier") {
        "glacierBackup".into()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::tz::TimeZone;

    use super::*;
    use crate::dsm::types;
    use crate::model::{BackupTask, Ids};
    use crate::service::testutil::{fixtures, svc};

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn list_from_fixture_with_missing_status() {
        let w = svc(fixtures())
            .list_backups(&BackupFilter::default())
            .unwrap();
        assert_eq!(
            w.data.items,
            vec![
                BackupTask {
                    id: "3".into(),
                    name: "Backup to LOCAL".into(),
                    kind: "hyperBackup".into(),
                    status: BackupTaskStatus::Idle,
                    last_result: BackupTaskResult::Success,
                },
                BackupTask {
                    id: "4".into(),
                    name: "Backup to REMOTE".into(),
                    kind: "hyperBackup".into(),
                    status: BackupTaskStatus::Idle,
                    last_result: BackupTaskResult::Unknown,
                },
            ]
        );
        assert_eq!(w.data.ids(), ["3", "4"]);
        assert_eq!(w.warnings.len(), 1);
        assert!(
            w.warnings[0].starts_with("backup task Backup to REMOTE: status unavailable: "),
            "{:?}",
            w.warnings
        );
    }

    #[test]
    fn get_from_fixture_in_nas_zone() {
        let mut s = svc(fixtures());
        s.tz = TimeZone::get("Europe/Warsaw").unwrap();
        let w = s.get_backup("3").unwrap();
        assert!(w.warnings.is_empty(), "{:?}", w.warnings);
        let d = w.data;
        assert_eq!(d.last_result, BackupTaskResult::Success);
        assert_eq!(d.last_run_at, Some(ts("2026-04-24T00:30:00Z")));
        assert_eq!(d.next_run_at, Some(ts("2026-04-24T23:00:00Z")));
        assert_eq!(d.size, Some(3206674163));
        assert_eq!(
            d.folders,
            Some(vec!["/volume1/docker".into(), "/volume1/Documents".into()])
        );
    }

    #[test]
    fn get_without_enrichment_omits_fields() {
        let w = svc(fixtures()).get_backup("4").unwrap();
        assert_eq!(
            (w.data.last_run_at, w.data.size, w.data.folders.clone()),
            (None, None, None)
        );
        assert_eq!(w.data.last_result, BackupTaskResult::Unknown);
        assert_eq!(w.warnings.len(), 3, "{:?}", w.warnings);
        let v = serde_json::to_value(&w.data).unwrap();
        assert!(v.get("lastRunAt").is_none() && v.get("size").is_none());
        assert_eq!(v["type"], "hyperBackup");
    }

    #[test]
    fn get_unknown_or_non_numeric_is_not_found() {
        for id in ["9", "abc", "nas-01.3", ""] {
            assert_eq!(
                svc(fixtures()).get_backup(id).unwrap_err().to_string(),
                format!("Backup task not found: {id}")
            );
        }
    }

    #[test]
    fn wide_list_has_details_per_task() {
        let w = svc(fixtures())
            .list_backups_wide(&BackupFilter::default())
            .unwrap();
        assert_eq!(w.data.len(), 2);
        assert_eq!(w.data[0].size, Some(3206674163));
        assert_eq!(w.data[1].size, None);
        assert_eq!(w.warnings.len(), 3);
    }

    #[test]
    fn list_filters_by_status_and_result() {
        let ids = |filter: BackupFilter| {
            let s = svc(fixtures());
            let list = s.list_backups(&filter).unwrap().data.ids();
            let wide: Vec<String> = s
                .list_backups_wide(&filter)
                .unwrap()
                .data
                .into_iter()
                .map(|d| d.task.id)
                .collect();
            assert_eq!(list, wide, "list and --wide filter alike");
            list
        };
        assert_eq!(
            ids(BackupFilter {
                result: Some(BackupTaskResult::Success),
                ..Default::default()
            }),
            ["3"]
        );
        assert_eq!(
            ids(BackupFilter {
                status: Some(BackupTaskStatus::Idle),
                result: Some(BackupTaskResult::Unknown),
            }),
            ["4"]
        );
        assert!(
            ids(BackupFilter {
                status: Some(BackupTaskStatus::Running),
                ..Default::default()
            })
            .is_empty()
        );
    }

    #[test]
    fn real_tasks_read_as_dsm_shows_them() {
        // 2026-10-25, DSM 7.1.1 / Hyper Backup 3.0.2. Hyper Backup showed
        // "Success" for the first and "Checking backup integrity... Waiting to
        // check server status" (target off-line) for the second.
        let tasks: types::BackupTaskList = serde_json::from_str(
            r#"{"task_list": [
                {"task_id": 3, "name": "Backup to LOCAL", "state": "backupable",
                 "status": "none", "type": "image:image_local"},
                {"task_id": 4, "name": "Backup to REMOTE", "state": "error_detect",
                 "status": "none", "type": "image:image_remote"}
            ]}"#,
        )
        .unwrap();
        let local: types::BackupTaskStatus = serde_json::from_str(
            r#"{"is_modified": false, "last_bkp_error": "", "last_bkp_error_code": 4401,
                "last_bkp_result": "done", "last_bkp_time": "2026/10/25 01:27",
                "state": "backupable", "status": "none", "task_id": 3}"#,
        )
        .unwrap();
        let remote: types::BackupTaskStatus = serde_json::from_str(
            r#"{"is_modified": false, "last_bkp_error": "", "last_bkp_error_code": 4401,
                "last_bkp_result": "failed", "last_bkp_time": "2026/10/25 03:25",
                "state": "error_detect", "status": "detect_waiting", "task_id": 4}"#,
        )
        .unwrap();
        let got: Vec<_> = [
            (&tasks.task_list[0], &local),
            (&tasks.task_list[1], &remote),
        ]
        .into_iter()
        .map(|(t, s)| {
            let t = task(t, Some(s));
            (t.status, t.last_result)
        })
        .collect();
        assert_eq!(
            got,
            [
                (BackupTaskStatus::Idle, BackupTaskResult::Success),
                (BackupTaskStatus::Checking, BackupTaskResult::Failed),
            ]
        );
    }

    #[test]
    fn hyper_backup_missing_is_an_error() {
        let mut api = fixtures();
        api.backup_tasks = None;
        assert!(matches!(
            svc(api).list_backups(&BackupFilter::default()),
            Err(crate::error::Error::NotInstalled(_))
        ));
    }

    #[test]
    fn result_maps_like_dsm() {
        use BackupTaskResult::*;
        let st = |r: &str, modified| types::BackupTaskStatus {
            last_bkp_result: r.into(),
            last_bkp_error_code: 4401,
            is_modified: modified,
            ..Default::default()
        };
        assert_eq!(backup_result(None), Unknown);
        for (raw, want) in [
            // The error code does not matter: DSM shows "Success".
            ("done", Success),
            ("success", Success),
            ("partial", Warning),
            ("cancel", Canceled),
            ("failed", Failed),
            ("discard", Failed),
            ("version_delete_failed", Failed),
            ("error", Failed),
            ("none", Unknown),
            ("failed_checking", Unknown),
            ("backingup", Unknown),
            ("resuming", Unknown),
            ("suspend", Unknown),
            ("", Unknown),
        ] {
            assert_eq!(backup_result(Some(&st(raw, false))), want, "{raw}");
        }
        for raw in ["done", "success", "partial"] {
            assert_eq!(backup_result(Some(&st(raw, true))), Unknown, "{raw}");
        }
        assert_eq!(backup_result(Some(&st("failed", true))), Failed);
    }

    #[test]
    fn status_maps_like_dsm() {
        use BackupTaskStatus::*;
        for (state, status, last, want) in [
            ("backupable", "none", Some("done"), Idle),
            ("backupable", "none", None, Idle),
            ("backupable", "none", Some("suspend"), Suspended),
            ("", "none", Some("done"), Idle),
            ("backupable", "backup", Some("done"), Running),
            ("backupable", "waiting", None, Running),
            ("backupable", "version_deleting", None, Running),
            ("backupable", "wait_discard", None, Running),
            ("backupable", "discard_failed", None, Error),
            // SNAIL: the integrity check waits for an offline target.
            ("error_detect", "none", Some("failed"), Checking),
            ("error_detect", "detect_waiting", Some("failed"), Checking),
            ("unauth", "none", None, Error),
            ("endofservice", "none", None, Error),
            ("broken", "none", None, Error),
            ("broken", "deleting", None, Running),
            ("restore_only", "none", None, Idle),
            ("exportable", "backup", None, Running),
            ("exportable", "none", Some("suspend"), Suspended),
            ("importable", "none", None, Idle),
            ("importable", "relink", None, Running),
            ("importable", "canceling", None, Error),
            ("relinkable", "relink", None, Running),
            ("relinkable", "none", None, Error),
            ("running", "none", None, Error),
        ] {
            assert_eq!(
                backup_status(state, status, last),
                want,
                "{state}/{status}/{last:?}"
            );
        }
    }

    #[test]
    fn type_maps() {
        assert_eq!(backup_type("image:image_local"), "hyperBackup");
        assert_eq!(backup_type("cloud_glacier"), "glacierBackup");
        assert_eq!(backup_type("rsync"), "rsync");
    }

    #[test]
    fn backup_time_parsing() {
        assert_eq!(
            parse_backup_time("2026/04/24 02:30", &TimeZone::UTC),
            Some(ts("2026-04-24T02:30:00Z"))
        );
        assert_eq!(parse_backup_time("", &TimeZone::UTC), None);
        assert_eq!(parse_backup_time("2026-04-24 02:30", &TimeZone::UTC), None);
    }
}
