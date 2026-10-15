//! `config check`: walks config → permissions → dns → tcp → tls → api →
//! auth → packages and reports the step where setup breaks.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use super::probe;
use crate::config::{self, Config};
use crate::dsm::client::{AccessError, DsmClient, TransportError};

const STEPS: [&str; 8] = [
    "config",
    "permissions",
    "dns",
    "tcp",
    "tls",
    "api",
    "auth",
    "packages",
];
const TLS_HINT: &str = "set insecure_tls = true (DSM ships a self-signed certificate)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub step: &'static str,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    pub fn new(step: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            step,
            status,
            detail: detail.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(checks: Vec<Check>) -> Self {
        let ok = checks.iter().all(|c| c.status != Status::Fail);
        Self { ok, checks }
    }
}

pub fn run(path: &Path, explicit: bool, env: &dyn Fn(&str) -> Option<String>) -> Report {
    run_with(path, explicit, env, "https")
}

/// `scheme` is `http` only in tests, against a plain-HTTP mock NAS.
pub(crate) fn run_with(
    path: &Path,
    explicit: bool,
    env: &dyn Fn(&str) -> Option<String>,
    scheme: &str,
) -> Report {
    let mut checks = Vec::new();
    steps(&mut checks, path, explicit, env, scheme);
    if let Some(failed) = checks
        .iter()
        .find(|c| c.status == Status::Fail)
        .map(|c| c.step)
    {
        for &step in &STEPS[checks.len()..] {
            checks.push(Check::new(
                step,
                Status::Skip,
                format!("skipped: {failed} failed"),
            ));
        }
    }
    Report::new(checks)
}

/// Pushes one check per step, stopping after the first failure.
fn steps(
    checks: &mut Vec<Check>,
    path: &Path,
    explicit: bool,
    env: &dyn Fn(&str) -> Option<String>,
    scheme: &str,
) -> Option<()> {
    let exists = path.is_file();
    let cfg = match config::load(path, explicit, env) {
        Ok(loaded) => {
            checks.push(Check::new(
                "config",
                Status::Pass,
                config_detail(path, exists, env),
            ));
            loaded.config
        }
        Err(e) => {
            let mut check = Check::new("config", Status::Fail, e.to_string());
            if !exists {
                check = check.hint("run `synoctl config init`");
            }
            checks.push(check);
            return None;
        }
    };
    checks.push(permissions(path, exists));

    let (host, port) = probe::split_host_port(&cfg.host);
    let addrs = gate(
        checks,
        match probe::resolve(&host, port) {
            Ok(addrs) => {
                let ips: Vec<String> = addrs.iter().map(|a| a.ip().to_string()).collect();
                Ok((
                    Check::new("dns", Status::Pass, format!("{host} → {}", ips.join(", "))),
                    addrs,
                ))
            }
            Err(e) => Err(Check::new(
                "dns",
                Status::Fail,
                format!("cannot resolve {host}: {e}"),
            )),
        },
    )?;
    gate(checks, match probe::connect(&addrs) {
        Ok(addr) => Ok((Check::new("tcp", Status::Pass, format!("connected to {addr}")), ())),
        Err(e) => Err(Check::new("tcp", Status::Fail, format!("cannot connect to port {port}: {e}"))
            .hint(format!(
                "is DSM listening on port {port}? its HTTPS port is 5001 unless changed: set host = \"{}\"",
                on_5001(&host)
            ))),
    })?;
    let client = match client(&cfg, scheme) {
        Ok(c) => c,
        Err(e) => {
            checks.push(Check::new("tls", Status::Fail, e.to_string()));
            return None;
        }
    };
    gate(checks, tls(&client, cfg.insecure_tls))?;
    let apis = gate(checks, api(&client, &host, port))?;
    gate(checks, auth(&client, &cfg.username))?;
    checks.push(packages(&apis));
    Some(())
}

/// Records the step's check and hands its value on, or stops the run.
fn gate<T>(checks: &mut Vec<Check>, result: Result<(Check, T), Check>) -> Option<T> {
    match result {
        Ok((check, value)) => {
            checks.push(check);
            Some(value)
        }
        Err(check) => {
            checks.push(check);
            None
        }
    }
}

fn config_detail(path: &Path, exists: bool, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut detail = if exists {
        path.display().to_string()
    } else {
        "no file, environment only".to_string()
    };
    let overrides = config::env_overrides(env);
    if !overrides.is_empty() {
        detail.push_str("; overrides: ");
        detail.push_str(&overrides.join(", "));
    }
    detail
}

fn permissions(path: &Path, exists: bool) -> Check {
    if !exists {
        return Check::new("permissions", Status::Skip, "no config file");
    }
    match config::file_mode(path) {
        None => Check::new("permissions", Status::Skip, "not checked on this platform"),
        Some(mode) if mode & 0o077 == 0 => {
            Check::new("permissions", Status::Pass, format!("{mode:04o}"))
        }
        Some(mode) => Check::new(
            "permissions",
            Status::Warn,
            format!("{mode:04o}: readable by other users"),
        )
        .hint(format!("chmod 600 {}", path.display())),
    }
}

/// A check should not hang for the 30 s a normal command tolerates.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A fresh client: no stored session and no cache, so `auth` really logs in.
fn client(cfg: &Config, scheme: &str) -> crate::error::Result<DsmClient> {
    Ok(DsmClient::with_base_url(
        format!("{scheme}://{}", cfg.host),
        cfg.host.clone(),
        cfg.username.clone(),
        cfg.password.clone(),
        cfg.auth_version,
        cfg.insecure_tls,
    )?
    .with_timeout(PROBE_TIMEOUT))
}

fn tls(client: &DsmClient, insecure_tls: bool) -> Result<(Check, ()), Check> {
    let fail = |detail: String| Check::new("tls", Status::Fail, detail);
    match client.handshake() {
        Ok(()) if insecure_tls => Ok((
            Check::new(
                "tls",
                Status::Warn,
                "certificate not verified (insecure_tls = true)",
            ),
            (),
        )),
        Ok(()) => Ok((Check::new("tls", Status::Pass, "certificate valid"), ())),
        Err(e @ TransportError::Certificate(_)) => Err(fail(e.to_string()).hint(TLS_HINT)),
        Err(e) => Err(fail(e.to_string())),
    }
}

/// `host` on DSM's HTTPS port, as the config's `host` value.
fn on_5001(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:5001")
    } else {
        format!("{host}:5001")
    }
}

fn api(client: &DsmClient, host: &str, port: u16) -> Result<(Check, BTreeSet<String>), Check> {
    match client.api_info() {
        Ok(apis) => Ok((
            Check::new(
                "api",
                Status::Pass,
                format!("DSM Web API answers ({} APIs)", apis.len()),
            ),
            apis,
        )),
        Err(AccessError::WebPage) if port != 5001 => Err(Check::new(
            "api",
            Status::Fail,
            format!("no DSM Web API: port {port} serves a web page"),
        )
        .hint(format!(
            "set host = \"{}\" (DSM's HTTPS port) unless DSM uses another port",
            on_5001(host)
        ))),
        Err(e) => Err(
            Check::new("api", Status::Fail, format!("no DSM Web API: {e}"))
                .hint("is this the NAS's DSM address and port?"),
        ),
    }
}

fn auth(client: &DsmClient, username: &str) -> Result<(Check, ()), Check> {
    let fail = |detail: String| Check::new("auth", Status::Fail, detail);
    match client.login_check() {
        Ok(()) => Ok((
            Check::new("auth", Status::Pass, format!("logged in as {username}")),
            (),
        )),
        Err(e @ AccessError::Login(403 | 404)) => Err(fail(e.to_string())
            .hint("use an account without 2-step verification, e.g. a dedicated synoctl user")),
        Err(e @ AccessError::Login(_)) => {
            Err(fail(e.to_string()).hint("check username and password"))
        }
        Err(e @ AccessError::Password(_)) => Err(fail(e.to_string())
            .hint("run the command yourself: it must print the password on its first line")),
        Err(e) => Err(fail(e.to_string())),
    }
}

fn packages(apis: &BTreeSet<String>) -> Check {
    let state = |api: &str| {
        if apis.contains(api) {
            "installed"
        } else {
            "not installed"
        }
    };
    Check::new(
        "packages",
        Status::Pass,
        format!(
            "Container Manager: {}, Hyper Backup: {}",
            state("SYNO.Docker.Container"),
            state("SYNO.Backup.Task")
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use httpmock::prelude::*;
    use serde_json::json;

    use super::Status::*;
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn write_cfg(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("config.toml");
        crate::private_fs::replace(&p, body).unwrap();
        p
    }

    fn statuses(r: &Report) -> Vec<(&'static str, Status)> {
        r.checks.iter().map(|c| (c.step, c.status)).collect()
    }

    fn check<'a>(r: &'a Report, step: &str) -> &'a Check {
        r.checks.iter().find(|c| c.step == step).unwrap()
    }

    fn cfg(dir: &tempfile::TempDir, server: &MockServer, extra: &str) -> PathBuf {
        write_cfg(
            dir,
            &format!(
                "host = \"{}\"\nusername = \"u\"\npassword = \"p\"\n{extra}",
                server.address()
            ),
        )
    }

    fn mock_api_info(server: &MockServer, apis: &[&str]) {
        let data: serde_json::Map<String, serde_json::Value> = apis
            .iter()
            .map(|a| (a.to_string(), json!({"path": "entry.cgi", "maxVersion": 1})))
            .collect();
        server.mock(move |w, t| {
            w.path("/webapi/query.cgi").query_param("query", "all");
            t.json_body(json!({"success": true, "data": data}));
        });
        server.mock(|w, t| {
            w.path("/webapi/query.cgi")
                .query_param("query", "SYNO.API.Auth");
            t.json_body(json!({"success": true, "data": {"SYNO.API.Auth": {"path": "entry.cgi"}}}));
        });
    }

    fn mock_login(server: &MockServer, body: serde_json::Value) {
        server.mock(move |w, t| {
            w.path("/webapi/entry.cgi").query_param("method", "login");
            t.json_body(body);
        });
    }

    #[test]
    fn all_steps_pass() {
        let server = MockServer::start();
        mock_api_info(&server, &["SYNO.API.Auth", "SYNO.Docker.Container"]);
        mock_login(&server, json!({"success": true, "data": {"sid": "S"}}));
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&cfg(&dir, &server, ""), false, &no_env, "http");
        assert!(r.ok, "{r:?}");
        assert_eq!(
            statuses(&r),
            vec![
                ("config", Pass),
                ("permissions", Pass),
                ("dns", Pass),
                ("tcp", Pass),
                ("tls", Pass),
                ("api", Pass),
                ("auth", Pass),
                ("packages", Pass),
            ]
        );
        assert_eq!(check(&r, "auth").detail, "logged in as u");
        assert_eq!(
            check(&r, "packages").detail,
            "Container Manager: installed, Hyper Backup: not installed"
        );
    }

    #[test]
    fn wrong_password_fails_auth_and_skips_packages() {
        let server = MockServer::start();
        mock_api_info(&server, &["SYNO.API.Auth"]);
        mock_login(&server, json!({"success": false, "error": {"code": 400}}));
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&cfg(&dir, &server, ""), false, &no_env, "http");
        assert!(!r.ok);
        let auth = check(&r, "auth");
        assert_eq!(auth.status, Fail);
        assert!(
            auth.detail.contains("wrong account or password"),
            "{}",
            auth.detail
        );
        assert_eq!(check(&r, "packages").status, Skip);
    }

    #[test]
    fn two_step_verification_hints_another_account() {
        let server = MockServer::start();
        mock_api_info(&server, &["SYNO.API.Auth"]);
        mock_login(&server, json!({"success": false, "error": {"code": 403}}));
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&cfg(&dir, &server, ""), false, &no_env, "http");
        assert!(
            check(&r, "auth")
                .hint
                .as_deref()
                .unwrap()
                .contains("2-step")
        );
    }

    #[test]
    fn non_dsm_server_fails_api() {
        let server = MockServer::start();
        server.mock(|w, t| {
            w.path("/webapi/query.cgi");
            t.status(404).body("not here");
        });
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&cfg(&dir, &server, ""), false, &no_env, "http");
        assert_eq!(check(&r, "api").status, Fail);
        assert_eq!(check(&r, "auth").status, Skip);
    }

    #[test]
    fn web_page_instead_of_api_hints_port_5001() {
        let server = MockServer::start();
        server.mock(|w, t| {
            w.path("/webapi/query.cgi");
            t.body("<!DOCTYPE html>\n<html><body>redirect</body></html>");
        });
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&cfg(&dir, &server, ""), false, &no_env, "http");
        let api = check(&r, "api");
        assert_eq!(api.status, Fail);
        assert!(api.detail.contains("web page"), "{}", api.detail);
        assert_eq!(
            api.hint.as_deref().unwrap(),
            "set host = \"127.0.0.1:5001\" (DSM's HTTPS port) unless DSM uses another port"
        );
    }

    #[test]
    fn refused_tcp_skips_the_rest() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(
            &dir,
            &format!("host = \"127.0.0.1:{port}\"\nusername = \"u\"\npassword = \"p\"\n"),
        );
        let r = run_with(&p, false, &no_env, "http");
        assert_eq!(
            statuses(&r)[3..],
            [
                ("tcp", Fail),
                ("tls", Skip),
                ("api", Skip),
                ("auth", Skip),
                ("packages", Skip)
            ]
        );
    }

    #[test]
    fn unresolvable_host() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_cfg(
            &dir,
            "host = \"synoctl-test.invalid\"\nusername = \"u\"\npassword = \"p\"\n",
        );
        let r = run_with(&p, false, &no_env, "http");
        assert_eq!(check(&r, "dns").status, Fail);
        assert_eq!(check(&r, "tcp").status, Skip);
    }

    #[test]
    fn missing_config_fails_first_step() {
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(&dir.path().join("none.toml"), false, &no_env, "http");
        let config = check(&r, "config");
        assert_eq!(config.status, Fail);
        assert_eq!(config.hint.as_deref(), Some("run `synoctl config init`"));
        assert!(r.checks[1..].iter().all(|c| c.status == Skip));
    }

    #[test]
    fn insecure_tls_is_a_warning() {
        let server = MockServer::start();
        mock_api_info(&server, &["SYNO.API.Auth"]);
        mock_login(&server, json!({"success": true, "data": {"sid": "S"}}));
        let dir = tempfile::tempdir().unwrap();
        let r = run_with(
            &cfg(&dir, &server, "insecure_tls = true\n"),
            false,
            &no_env,
            "http",
        );
        assert!(r.ok);
        assert_eq!(check(&r, "tls").status, Warn);
    }

    #[test]
    fn json_shape() {
        let r = Report::new(vec![Check::new("tcp", Fail, "refused").hint("h")]);
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"ok": false, "checks": [{"step": "tcp", "status": "fail", "detail": "refused", "hint": "h"}]})
        );
    }
}
