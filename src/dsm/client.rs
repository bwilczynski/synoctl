//! HTTP access to the DSM Web API, ported from
//! homelab-api/internal/adapters/synology.go.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use reqwest::blocking::Client;
use serde::de::DeserializeOwned;

use super::session::SessionStore;
use super::types::{
    BackupTarget, BackupTaskDetail, BackupTaskList, BackupTaskStatus, ContainerDetailResponse,
    ContainerList, ContainerResources, Envelope, ImageList, NetworkList, Storage, SystemInfo,
    Utilization,
};
use super::{DsmApi, backup_missing, container_not_found, docker_missing};
use crate::cache::Cache;
use crate::config::{Config, Password};
use crate::error::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(30);

/// DSM codes meaning the SID is no longer accepted: session timeout,
/// interrupted by a duplicate login, SID not found.
const SESSION_EXPIRED: [i64; 3] = [106, 107, 119];
/// "The requested API does not exist": the package providing it is missing.
const API_MISSING: i64 = 102;
/// `SYNO.Docker.Container`: no such container.
const CONTAINER_NOT_FOUND: i64 = 117;

const STATUS_ADDITIONAL: &str = r#"["last_bkp_time","next_bkp_time","last_bkp_result","is_modified","last_bkp_progress","last_bkp_success_version"]"#;
const TASK_ADDITIONAL: &str = r#"["repository","schedule"]"#;
const TARGET_ADDITIONAL: &str =
    r#"["is_online","used_size","check_task_key","check_auth","account_meta"]"#;

pub struct DsmClient {
    base_url: String,
    host: String,
    username: String,
    password: Password,
    /// `password` once resolved, so a run asks a password manager only once.
    resolved_password: OnceLock<String>,
    auth_version: u32,
    http: Client,
    cache: Option<Cache>,
    session: Option<SessionStore>,
    /// The SID in use; locked for the duration of a login, so parallel reads
    /// share one login.
    sid: Mutex<Option<String>>,
    timeout: Duration,
}

/// Why a request never got an HTTP response, for `config check`/`config init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Dns(String),
    Connect(String),
    Timeout,
    /// The NAS's certificate was rejected; holds the reason (e.g. `UnknownIssuer`).
    Certificate(String),
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dns(m) => write!(f, "DNS lookup failed: {m}"),
            Self::Connect(m) => write!(f, "connection failed: {m}"),
            Self::Timeout => write!(f, "timed out"),
            Self::Certificate(r) => write!(f, "certificate not trusted: {r}"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

/// Why a setup probe failed: the request, the login, or the response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    Transport(TransportError),
    /// DSM's login error code.
    Login(i64),
    /// `password_command` failed; the message names it.
    Password(String),
    Status(u16),
    /// An HTML page where DSM's JSON was expected: usually the wrong port.
    WebPage,
    Parse(String),
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Login(code) => write!(f, "{}", login_message(*code)),
            Self::Password(m) => write!(f, "{m}"),
            Self::Status(code) => write!(f, "status {code}"),
            Self::WebPage => write!(f, "a web page, not the DSM Web API"),
            Self::Parse(m) => write!(f, "unexpected response: {m}"),
        }
    }
}

/// How a DSM call failed, before it becomes a user-facing `Error`.
enum CallError {
    /// `success: false` with this code.
    Code(i64),
    Other(Error),
}

impl DsmClient {
    pub fn new(cfg: &Config) -> Result<Self> {
        Self::with_base_url(
            format!("https://{}", cfg.host),
            cfg.host.clone(),
            cfg.username.clone(),
            cfg.password.clone(),
            cfg.auth_version,
            cfg.insecure_tls,
        )
    }

    pub fn with_base_url(
        base_url: String,
        host: String,
        username: String,
        password: Password,
        auth_version: u32,
        insecure_tls: bool,
    ) -> Result<Self> {
        let http = Client::builder()
            // Never follow redirects: the password and SID travel in the query.
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(insecure_tls)
            .build()
            .map_err(|e| Error::Dsm(format!("synology client: {}", describe(&e))))?;
        Ok(Self {
            base_url,
            host,
            username,
            password,
            resolved_password: OnceLock::new(),
            auth_version,
            http,
            cache: None,
            session: None,
            sid: Mutex::new(None),
            timeout: TIMEOUT,
        })
    }

    /// Serves fresh responses from `cache` and stores new ones in it.
    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Reuses the SID stored by an earlier run and stores new ones.
    pub fn with_session_store(mut self, store: SessionStore) -> Self {
        self.session = Some(store);
        self
    }

    /// Gives up on each request after `timeout` instead of the default 30 s.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// One unauthenticated request; any HTTP response means DNS, TCP and TLS
    /// all worked.
    pub fn handshake(&self) -> std::result::Result<(), TransportError> {
        self.http
            .get(format!("{}/webapi/query.cgi", self.base_url))
            .timeout(self.timeout)
            .send()
            .map(drop)
            .map_err(|e| classify(&e))
    }

    /// Every API the NAS offers (`SYNO.API.Info` `query=all`), uncached.
    pub fn api_info(&self) -> std::result::Result<BTreeSet<String>, AccessError> {
        let data: serde_json::Map<String, serde_json::Value> =
            self.info_query("all").map_err(|e| match e {
                CallError::Code(c) => AccessError::Parse(format!("error code {c}")),
                CallError::Other(Error::Dsm(m)) if m == WEB_PAGE => AccessError::WebPage,
                CallError::Other(e) => AccessError::Parse(e.to_string()),
            })?;
        Ok(data.keys().cloned().collect())
    }

    /// A fresh login (not the stored SID), for `config check`; the new SID
    /// is kept and stored.
    pub fn login_check(&self) -> std::result::Result<(), AccessError> {
        let mut sid = self.sid.lock().unwrap_or_else(PoisonError::into_inner);
        let new = self.login_raw().map_err(|e| match e {
            CallError::Code(c) => AccessError::Login(c),
            CallError::Other(e @ Error::Auth(_)) => AccessError::Password(e.to_string()),
            CallError::Other(e) => AccessError::Parse(e.to_string()),
        })?;
        self.remember(&new);
        *sid = Some(new);
        Ok(())
    }

    /// GET `webapi/{path}` with `params`; the parsed envelope.
    fn send<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<(String, Envelope<T>), CallError> {
        let url =
            reqwest::Url::parse_with_params(&format!("{}/webapi/{path}", self.base_url), params)
                .map_err(|e| CallError::Other(Error::Dsm(format!("synology request: {e}"))))?;
        let resp = self
            .http
            .get(url)
            .timeout(self.timeout)
            .send()
            .map_err(transport)?;
        if !resp.status().is_success() {
            return Err(CallError::Other(Error::Dsm(format!(
                "synology request: status {}",
                resp.status().as_u16()
            ))));
        }
        let body = resp.text().map_err(transport)?;
        let env = parse::<T>(&body).map_err(CallError::Other)?;
        Ok((body, env))
    }

    /// `SYNO.API.Info` `query` for `query`; the data object.
    fn info_query<T: DeserializeOwned>(&self, query: &str) -> std::result::Result<T, CallError> {
        let params = [
            ("api", "SYNO.API.Info"),
            ("version", "1"),
            ("method", "query"),
            ("query", query),
        ];
        let (_, env) = self.send::<T>("query.cgi", &params)?;
        data(env)
    }

    /// Logs in and returns the new SID.
    fn login_raw(&self) -> std::result::Result<String, CallError> {
        #[derive(serde::Deserialize)]
        struct ApiInfo {
            path: String,
        }
        #[derive(serde::Deserialize)]
        struct Login {
            sid: String,
        }
        let mut apis: std::collections::HashMap<String, ApiInfo> =
            self.info_query("SYNO.API.Auth")?;
        let path = apis
            .remove("SYNO.API.Auth")
            .map(|a| a.path)
            .ok_or_else(|| {
                CallError::Other(Error::Dsm(
                    "synology login: SYNO.API.Auth not found in API info".into(),
                ))
            })?;
        let version = self.auth_version.to_string();
        let password = self.password().map_err(CallError::Other)?;
        let params = [
            ("api", "SYNO.API.Auth"),
            ("method", "login"),
            ("version", version.as_str()),
            ("account", self.username.as_str()),
            ("passwd", password),
            ("format", "sid"),
        ];
        let (_, env) = self.send::<Login>(&path, &params)?;
        data(env).map(|l| l.sid)
    }

    /// The password, running `password_command` on first use only.
    fn password(&self) -> Result<&str> {
        if let Some(p) = self.resolved_password.get() {
            return Ok(p);
        }
        let p = self.password.resolve()?;
        Ok(self.resolved_password.get_or_init(|| p))
    }

    fn remember(&self, sid: &str) {
        if let Some(store) = &self.session {
            store.store(&self.host, &self.username, sid);
        }
    }

    /// The SID to use: the current one unless it is `stale`, else the stored
    /// one (first call only), else a new login.
    fn session_id(&self, stale: Option<&str>) -> Result<String> {
        let mut sid = self.sid.lock().unwrap_or_else(PoisonError::into_inner);
        match (sid.as_deref(), stale) {
            (Some(current), Some(old)) if current != old => return Ok(current.to_string()),
            (Some(current), None) => return Ok(current.to_string()),
            _ => {}
        }
        if stale.is_none()
            && let Some(stored) = self
                .session
                .as_ref()
                .and_then(|s| s.load(&self.host, &self.username))
        {
            *sid = Some(stored.clone());
            return Ok(stored);
        }
        let new = self.login_raw().map_err(|e| match e {
            CallError::Code(code) => login_error(code),
            CallError::Other(e) => e,
        })?;
        self.remember(&new);
        *sid = Some(new.clone());
        Ok(new)
    }

    /// One authenticated `entry.cgi` call; the raw body of a successful
    /// response. Logs in again and retries once when the session expired.
    fn authed(
        &self,
        api: &str,
        method: &str,
        version: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<String, CallError> {
        let call = |sid: &str| {
            let mut all = vec![("api", api), ("method", method), ("version", version)];
            all.extend_from_slice(params);
            all.push(("_sid", sid));
            let (body, env) = self.send::<serde_json::Value>("entry.cgi", &all)?;
            data(env).map(|_| body)
        };
        let sid = self.session_id(None).map_err(CallError::Other)?;
        match call(&sid) {
            Err(CallError::Code(code)) if SESSION_EXPIRED.contains(&code) => {
                let sid = self.session_id(Some(&sid)).map_err(CallError::Other)?;
                call(&sid)
            }
            other => other,
        }
    }

    /// A read: from the cache when fresh, else from the NAS (and cached).
    fn read<T: DeserializeOwned>(
        &self,
        api: &str,
        method: &str,
        version: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        self.read_raw(api, method, version, params)
            .map_err(|e| self.call_error(api, e))
    }

    fn read_raw<T: DeserializeOwned>(
        &self,
        api: &str,
        method: &str,
        version: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<T, CallError> {
        let key = self.cache_key(api, method, version, params);
        if let Some(env) = self
            .cache
            .as_ref()
            .and_then(|c| c.get(&key))
            .and_then(|body| parse::<T>(&body).ok())
            && let Ok(value) = data(env)
        {
            return Ok(value);
        }
        let body = self.authed(api, method, version, params)?;
        let value = data(parse::<T>(&body).map_err(CallError::Other)?)?;
        if let Some(cache) = &self.cache {
            cache.put(&key, &body);
        }
        Ok(value)
    }

    fn cache_key(&self, api: &str, method: &str, version: &str, params: &[(&str, &str)]) -> String {
        let mut sorted = params.to_vec();
        sorted.sort();
        let query: Vec<String> = sorted.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!(
            "{}@{}/{api}.{method}.v{version}?{}",
            self.username,
            self.host,
            query.join("&")
        )
    }

    fn call_error(&self, api: &str, e: CallError) -> Error {
        match e {
            CallError::Other(e) => e,
            CallError::Code(API_MISSING) if api.starts_with("SYNO.Docker.") => {
                docker_missing(&self.host)
            }
            CallError::Code(API_MISSING) if api.starts_with("SYNO.Backup.") => {
                backup_missing(&self.host)
            }
            CallError::Code(code) => Error::Dsm(format!("synology API {api} error code {code}")),
        }
    }

    /// A container read or action, mapping "no such container".
    fn on_container<T>(&self, name: &str, r: std::result::Result<T, CallError>) -> Result<T> {
        match r {
            Err(CallError::Code(CONTAINER_NOT_FOUND)) => Err(container_not_found(name)),
            other => other.map_err(|e| self.call_error("SYNO.Docker.Container", e)),
        }
    }

    fn container_action(&self, method: &str, name: &str) -> Result<()> {
        let r = self.authed("SYNO.Docker.Container", method, "1", &[("name", name)]);
        self.on_container(name, r)?;
        if let Some(cache) = &self.cache {
            for (api, method, params) in [
                ("SYNO.Docker.Container", "list", CONTAINERS_PARAMS),
                ("SYNO.Docker.Container", "get", &[("name", name)][..]),
                ("SYNO.Docker.Container.Resource", "get", &[][..]),
            ] {
                cache.remove(&self.cache_key(api, method, "1", params));
            }
        }
        Ok(())
    }

    fn backup_read<T: DeserializeOwned>(
        &self,
        api: &str,
        method: &str,
        task_id: i64,
        extra: &[(&str, &str)],
    ) -> Result<T> {
        let id = task_id.to_string();
        let mut params = vec![("task_id", id.as_str())];
        params.extend_from_slice(extra);
        self.read(api, method, "1", &params)
    }
}

const CONTAINERS_PARAMS: &[(&str, &str)] = &[("limit", "0"), ("offset", "0")];

/// A failed request, without its URL: DSM takes the password and the SID
/// as query parameters, and reqwest's message would show them.
fn transport(e: reqwest::Error) -> CallError {
    CallError::Other(Error::Dsm(format!(
        "synology request: {}",
        describe(&e.without_url())
    )))
}

/// What a port serving a web page instead of the Web API reads as; on a
/// bare host that is DSM's port 443, a page redirecting browsers to 5001.
const WEB_PAGE: &str = "synology response is a web page, not the DSM Web API: \
                        is the port right? DSM's HTTPS port is 5001 unless changed";

fn parse<T: DeserializeOwned>(body: &str) -> Result<Envelope<T>> {
    if body.trim_start().starts_with('<') {
        return Err(Error::Dsm(WEB_PAGE.into()));
    }
    serde_json::from_str(body).map_err(|e| Error::Dsm(format!("synology parse response: {e}")))
}

/// The data of a successful envelope; its error code otherwise. A success
/// without data (container actions) reads as `null`.
fn data<T: DeserializeOwned>(env: Envelope<T>) -> std::result::Result<T, CallError> {
    if !env.success {
        return Err(CallError::Code(env.error.map_or(0, |e| e.code)));
    }
    match env.data {
        Some(d) => Ok(d),
        None => serde_json::from_value(serde_json::Value::Null)
            .map_err(|e| CallError::Other(Error::Dsm(format!("synology parse response: {e}")))),
    }
}

fn login_message(code: i64) -> String {
    match code {
        400 => "synology login: wrong account or password (error 400)".into(),
        401 => "synology login: account disabled (error 401)".into(),
        402 => "synology login: permission denied (error 402)".into(),
        403 | 404 => format!(
            "synology login: 2-step verification is enabled for this account, which synoctl does not support (error {code})"
        ),
        _ => format!("synology login failed: error code {code}"),
    }
}

fn login_error(code: i64) -> Error {
    match code {
        400..=404 => Error::Auth(login_message(code)),
        _ => Error::Dsm(login_message(code)),
    }
}

impl DsmApi for DsmClient {
    fn host(&self) -> &str {
        &self.host
    }

    fn system_info(&self) -> Result<SystemInfo> {
        self.read("SYNO.Core.System", "info", "1", &[])
    }

    fn system_utilization(&self) -> Result<Utilization> {
        self.read("SYNO.Core.System.Utilization", "get", "1", &[])
    }

    fn storage(&self) -> Result<Storage> {
        self.read("SYNO.Storage.CGI.Storage", "load_info", "1", &[])
    }

    fn containers(&self) -> Result<ContainerList> {
        self.read("SYNO.Docker.Container", "list", "1", CONTAINERS_PARAMS)
    }

    fn container(&self, name: &str) -> Result<ContainerDetailResponse> {
        let r = self.read_raw("SYNO.Docker.Container", "get", "1", &[("name", name)]);
        self.on_container(name, r)
    }

    fn container_resources(&self) -> Result<ContainerResources> {
        self.read("SYNO.Docker.Container.Resource", "get", "1", &[])
    }

    fn start_container(&self, name: &str) -> Result<()> {
        self.container_action("start", name)
    }

    fn stop_container(&self, name: &str) -> Result<()> {
        self.container_action("stop", name)
    }

    fn restart_container(&self, name: &str) -> Result<()> {
        self.container_action("restart", name)
    }

    fn images(&self) -> Result<ImageList> {
        let params = [("limit", "-1"), ("offset", "0"), ("show_dsm", "false")];
        self.read("SYNO.Docker.Image", "list", "1", &params)
    }

    fn networks(&self) -> Result<NetworkList> {
        self.read("SYNO.Docker.Network", "list", "1", &[])
    }

    fn backup_tasks(&self) -> Result<BackupTaskList> {
        self.read("SYNO.Backup.Task", "list", "1", &[])
    }

    fn backup_task(&self, task_id: i64) -> Result<BackupTaskDetail> {
        self.backup_read(
            "SYNO.Backup.Task",
            "get",
            task_id,
            &[("additional", TASK_ADDITIONAL)],
        )
    }

    fn backup_status(&self, task_id: i64) -> Result<BackupTaskStatus> {
        let extra = [("blOnline", "false"), ("additional", STATUS_ADDITIONAL)];
        self.backup_read("SYNO.Backup.Task", "status", task_id, &extra)
    }

    fn backup_target(&self, task_id: i64) -> Result<BackupTarget> {
        let extra = [("additional", TARGET_ADDITIONAL)];
        self.backup_read("SYNO.Backup.Target", "get", task_id, &extra)
    }
}

/// reqwest's Display omits the cause ("error sending request"); append the
/// source chain so TLS and DNS failures are visible.
fn chain(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

fn describe(e: &reqwest::Error) -> String {
    with_tls_hint(chain(e))
}

pub fn classify(e: &reqwest::Error) -> TransportError {
    classify_chain(&chain(e), e.is_timeout(), e.is_connect())
}

/// Sorts a failed request by the text of its error chain. rustls reports
/// `invalid peer certificate: <Reason>`; hyper reports `dns error: …`.
fn classify_chain(chain: &str, timeout: bool, connect: bool) -> TransportError {
    let lower = chain.to_lowercase();
    if lower.contains("certificate") {
        let reason = chain.rsplit(": ").next().unwrap_or(chain);
        TransportError::Certificate(reason.to_string())
    } else if timeout {
        TransportError::Timeout
    } else if lower.contains("dns error") {
        TransportError::Dns(chain.to_string())
    } else if connect {
        TransportError::Connect(chain.to_string())
    } else {
        TransportError::Other(chain.to_string())
    }
}

/// DSM ships a self-signed certificate; point users at the fix.
pub(crate) fn with_tls_hint(msg: String) -> String {
    if msg.to_lowercase().contains("certificate") {
        format!(
            "{msg} (set insecure_tls = true or SYNO_INSECURE_TLS=1 for the NAS's self-signed certificate)"
        )
    } else {
        msg
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use httpmock::prelude::*;
    use serde_json::json;

    use super::*;

    const ENTRY: &str = "/webapi/entry.cgi";

    fn client(server: &MockServer) -> DsmClient {
        DsmClient::with_base_url(
            server.base_url(),
            "nas".into(),
            "u".into(),
            Password::Literal("p".into()),
            6,
            false,
        )
        .unwrap()
    }

    fn with_store(server: &MockServer, dir: &tempfile::TempDir) -> DsmClient {
        client(server).with_session_store(SessionStore::new(dir.path()))
    }

    fn mock_auth(server: &MockServer) -> httpmock::Mock<'_> {
        server.mock(|w, t| {
            w.method(GET)
                .path("/webapi/query.cgi")
                .query_param("api", "SYNO.API.Info")
                .query_param("query", "SYNO.API.Auth");
            t.json_body(json!({"success": true, "data": {
                "SYNO.API.Auth": {"path": "entry.cgi", "minVersion": 1, "maxVersion": 7}}}));
        })
    }

    fn mock_login<'a>(server: &'a MockServer, sid: &str) -> httpmock::Mock<'a> {
        let sid = sid.to_string();
        server.mock(move |w, t| {
            w.method(GET)
                .path(ENTRY)
                .query_param("api", "SYNO.API.Auth")
                .query_param("method", "login")
                .query_param("version", "6")
                .query_param("account", "u")
                .query_param("passwd", "p")
                .query_param("format", "sid");
            t.json_body(json!({"success": true, "data": {"sid": sid}}));
        })
    }

    fn mock_login_error(server: &MockServer, code: i64) -> httpmock::Mock<'_> {
        server.mock(move |w, t| {
            w.path(ENTRY).query_param("api", "SYNO.API.Auth");
            t.json_body(json!({"success": false, "error": {"code": code}}));
        })
    }

    fn mock_call<'a>(
        server: &'a MockServer,
        api: &str,
        method: &str,
        sid: &str,
        body: serde_json::Value,
    ) -> httpmock::Mock<'a> {
        let (api, method, sid) = (api.to_string(), method.to_string(), sid.to_string());
        server.mock(move |w, t| {
            w.method(GET)
                .path(ENTRY)
                .query_param("api", api)
                .query_param("method", method)
                .query_param("_sid", sid);
            t.json_body(body);
        })
    }

    fn info_ok() -> serde_json::Value {
        json!({"success": true, "data": {"model": "DS918+", "up_time": "1:00:00"}})
    }

    fn code(c: i64) -> serde_json::Value {
        json!({"success": false, "error": {"code": c}})
    }

    #[test]
    fn logs_in_then_calls_with_sid() {
        let server = MockServer::start();
        let auth = mock_auth(&server);
        let login = mock_login(&server, "S1");
        let info = mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        assert_eq!(client(&server).system_info().unwrap().model, "DS918+");
        auth.assert_calls(1);
        login.assert_calls(1);
        info.assert_calls(1);
    }

    #[test]
    fn parallel_reads_share_one_login() {
        let server = MockServer::start();
        mock_auth(&server);
        let login = mock_login(&server, "S1");
        mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        let c = client(&server);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| c.system_info().unwrap());
            }
        });
        login.assert_calls(1);
    }

    #[test]
    fn stored_sid_skips_login() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        SessionStore::new(dir.path()).store("nas", "u", "S0");
        let login = mock_login(&server, "S1");
        let info = mock_call(&server, "SYNO.Core.System", "info", "S0", info_ok());
        with_store(&server, &dir).system_info().unwrap();
        login.assert_calls(0);
        info.assert_calls(1);
    }

    #[test]
    fn new_sid_is_stored() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        mock_auth(&server);
        mock_login(&server, "S1");
        mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        with_store(&server, &dir).system_info().unwrap();
        assert_eq!(
            SessionStore::new(dir.path()).load("nas", "u").as_deref(),
            Some("S1")
        );
    }

    #[test]
    fn foreign_or_corrupt_session_file_is_ignored() {
        for setup in [
            |d: &Path| std::fs::write(d.join("session.json"), "garbage").unwrap(),
            |d: &Path| SessionStore::new(d).store("other", "u", "S0"),
        ] {
            let server = MockServer::start();
            let dir = tempfile::tempdir().unwrap();
            setup(dir.path());
            mock_auth(&server);
            let login = mock_login(&server, "S1");
            mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
            with_store(&server, &dir).system_info().unwrap();
            login.assert_calls(1);
        }
    }

    #[test]
    fn expired_session_logs_in_again_and_retries_once() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        SessionStore::new(dir.path()).store("nas", "u", "OLD");
        mock_auth(&server);
        let login = mock_login(&server, "NEW");
        let old = mock_call(&server, "SYNO.Core.System", "info", "OLD", code(119));
        let new = mock_call(&server, "SYNO.Core.System", "info", "NEW", info_ok());
        assert_eq!(
            with_store(&server, &dir).system_info().unwrap().model,
            "DS918+"
        );
        login.assert_calls(1);
        old.assert_calls(1);
        new.assert_calls(1);
        assert_eq!(
            SessionStore::new(dir.path()).load("nas", "u").as_deref(),
            Some("NEW")
        );
    }

    #[test]
    fn second_expiry_is_an_error_not_a_loop() {
        let server = MockServer::start();
        mock_auth(&server);
        let login = mock_login(&server, "S1");
        let info = mock_call(&server, "SYNO.Core.System", "info", "S1", code(106));
        let e = client(&server).system_info().unwrap_err();
        assert_eq!(
            e.to_string(),
            "synology API SYNO.Core.System error code 106"
        );
        login.assert_calls(2);
        info.assert_calls(2);
    }

    #[test]
    fn wrong_password_is_auth_error() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login_error(&server, 400);
        let e = client(&server).system_info().unwrap_err();
        assert!(matches!(e, Error::Auth(_)), "{e:?}");
        assert_eq!(
            e.to_string(),
            "synology login: wrong account or password (error 400)"
        );
    }

    /// A client whose password comes from `cmd`.
    fn with_command(server: &MockServer, cmd: String) -> DsmClient {
        DsmClient::with_base_url(
            server.base_url(),
            "nas".into(),
            "u".into(),
            Password::Command(cmd),
            6,
            false,
        )
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn password_command_runs_once_and_only_to_log_in() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        let cmd = format!("echo x >> '{}'; echo p", runs.display());
        mock_auth(&server);
        let login = mock_login(&server, "S1");
        mock_call(&server, "SYNO.Core.System", "info", "S1", code(106));
        // Both logins (first call and the retry) share one run.
        with_command(&server, cmd.clone())
            .system_info()
            .unwrap_err();
        login.assert_calls(2);
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "x\n");

        let server = MockServer::start();
        let store = tempfile::tempdir().unwrap();
        SessionStore::new(store.path()).store("nas", "u", "S1");
        mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        with_command(&server, cmd)
            .with_session_store(SessionStore::new(store.path()))
            .system_info()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&runs).unwrap(), "x\n");
    }

    #[cfg(unix)]
    #[test]
    fn failed_password_command_stops_the_login() {
        let server = MockServer::start();
        mock_auth(&server);
        let login = mock_login(&server, "S1");
        let e = with_command(&server, "exit 1".into())
            .system_info()
            .unwrap_err();
        assert!(matches!(e, Error::Auth(_)), "{e:?}");
        assert_eq!(
            e.to_string(),
            "password_command `exit 1`: exited with status 1"
        );
        login.assert_calls(0);
        assert_eq!(
            with_command(&server, "exit 1".into()).login_check(),
            Err(AccessError::Password(
                "password_command `exit 1`: exited with status 1".into()
            ))
        );
    }

    #[test]
    fn two_step_verification_is_explained() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login_error(&server, 403);
        let e = client(&server).system_info().unwrap_err().to_string();
        assert!(e.contains("2-step verification"), "{e}");
    }

    #[test]
    fn container_manager_missing_is_not_installed() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        mock_call(&server, "SYNO.Docker.Image", "list", "S1", code(102));
        let e = client(&server).images().unwrap_err();
        assert!(matches!(e, Error::NotInstalled(_)));
        assert_eq!(e.to_string(), "Container Manager is not installed on nas");
    }

    #[test]
    fn hyper_backup_missing_is_not_installed() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        mock_call(&server, "SYNO.Backup.Task", "list", "S1", code(102));
        assert_eq!(
            client(&server).backup_tasks().unwrap_err().to_string(),
            "Hyper Backup is not installed on nas"
        );
    }

    #[test]
    fn unknown_container_is_not_found() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        mock_call(&server, "SYNO.Docker.Container", "get", "S1", code(117));
        mock_call(&server, "SYNO.Docker.Container", "stop", "S1", code(117));
        let c = client(&server);
        assert_eq!(
            c.container("x").unwrap_err().to_string(),
            "Container not found: x"
        );
        assert_eq!(
            c.stop_container("x").unwrap_err().to_string(),
            "Container not found: x"
        );
    }

    #[test]
    fn other_codes_name_api_and_code() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        mock_call(&server, "SYNO.Core.System", "info", "S1", code(105));
        assert_eq!(
            client(&server).system_info().unwrap_err().to_string(),
            "synology API SYNO.Core.System error code 105"
        );
    }

    #[test]
    fn http_errors_and_bad_json() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        server.mock(|w, t| {
            w.path(ENTRY).query_param("api", "SYNO.Core.System");
            t.status(502);
        });
        server.mock(|w, t| {
            w.path(ENTRY).query_param("api", "SYNO.Storage.CGI.Storage");
            t.body("<html>");
        });
        let c = client(&server);
        assert_eq!(
            c.system_info().unwrap_err().to_string(),
            "synology request: status 502"
        );
        assert_eq!(
            c.storage().unwrap_err().to_string(),
            "synology response is a web page, not the DSM Web API: \
             is the port right? DSM's HTTPS port is 5001 unless changed"
        );
    }

    fn cached(server: &MockServer, dir: &tempfile::TempDir) -> DsmClient {
        client(server).with_cache(Cache::new(
            dir.path().join("cache"),
            Duration::from_secs(60),
            true,
        ))
    }

    #[test]
    fn reads_are_cached_without_sid_in_key() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        mock_auth(&server);
        mock_login(&server, "S1");
        let info = mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        let c = cached(&server, &dir);
        c.system_info().unwrap();
        c.system_info().unwrap();
        info.assert_calls(1);
        for f in std::fs::read_dir(dir.path().join("cache")).unwrap() {
            let name = f.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.contains("S1"), "{name}");
        }
    }

    #[test]
    fn cached_reads_are_not_shared_between_accounts() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        mock_auth(&server);
        mock_login(&server, "S1");
        let info = mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        cached(&server, &dir).system_info().unwrap();
        server.mock(|w, t| {
            w.path(ENTRY)
                .query_param("method", "login")
                .query_param("account", "v");
            t.json_body(json!({"success": true, "data": {"sid": "S1"}}));
        });
        let mut other = cached(&server, &dir);
        other.username = "v".into();
        other.system_info().unwrap();
        info.assert_calls(2);
    }

    #[test]
    fn failure_responses_are_not_cached() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        mock_auth(&server);
        mock_login(&server, "S1");
        let mut fail = mock_call(&server, "SYNO.Core.System", "info", "S1", code(105));
        let c = cached(&server, &dir);
        assert!(c.system_info().is_err());
        fail.delete();
        let ok = mock_call(&server, "SYNO.Core.System", "info", "S1", info_ok());
        c.system_info().unwrap();
        ok.assert_calls(1);
    }

    #[test]
    fn container_action_drops_cached_container_reads() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        mock_auth(&server);
        mock_login(&server, "S1");
        let list = mock_call(
            &server,
            "SYNO.Docker.Container",
            "list",
            "S1",
            json!({"success": true, "data": {"containers": []}}),
        );
        let restart = mock_call(
            &server,
            "SYNO.Docker.Container",
            "restart",
            "S1",
            json!({"success": true}),
        );
        let c = cached(&server, &dir);
        c.containers().unwrap();
        c.containers().unwrap();
        list.assert_calls(1);
        c.restart_container("x").unwrap();
        restart.assert_calls(1);
        c.containers().unwrap();
        list.assert_calls(2);
    }

    #[test]
    fn request_params_match_homelab_api() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login(&server, "S1");
        let ok = json!({"success": true, "data": {}});
        let m = |api: &'static str,
                 method: &'static str,
                 params: &'static [(&'static str, &'static str)]| {
            let ok = ok.clone();
            server.mock(move |w, t| {
                let mut w = w
                    .path(ENTRY)
                    .query_param("api", api)
                    .query_param("method", method);
                for (k, v) in params {
                    w = w.query_param(*k, *v);
                }
                t.json_body(ok);
            })
        };
        let mocks = [
            m(
                "SYNO.Docker.Container",
                "list",
                &[("version", "1"), ("limit", "0"), ("offset", "0")],
            ),
            m(
                "SYNO.Docker.Image",
                "list",
                &[("limit", "-1"), ("offset", "0"), ("show_dsm", "false")],
            ),
            m("SYNO.Docker.Container", "get", &[("name", "immich_server")]),
            m(
                "SYNO.Backup.Task",
                "status",
                &[
                    ("task_id", "3"),
                    ("blOnline", "false"),
                    (
                        "additional",
                        r#"["last_bkp_time","next_bkp_time","last_bkp_result","is_modified","last_bkp_progress","last_bkp_success_version"]"#,
                    ),
                ],
            ),
            m(
                "SYNO.Backup.Task",
                "get",
                &[
                    ("task_id", "3"),
                    ("additional", r#"["repository","schedule"]"#),
                ],
            ),
            m(
                "SYNO.Backup.Target",
                "get",
                &[
                    ("task_id", "3"),
                    (
                        "additional",
                        r#"["is_online","used_size","check_task_key","check_auth","account_meta"]"#,
                    ),
                ],
            ),
            m("SYNO.Storage.CGI.Storage", "load_info", &[("version", "1")]),
            m("SYNO.Core.System.Utilization", "get", &[("version", "1")]),
            m("SYNO.Docker.Container.Resource", "get", &[("version", "1")]),
            m("SYNO.Docker.Network", "list", &[("version", "1")]),
        ];
        let c = client(&server);
        c.containers().unwrap();
        c.images().unwrap();
        c.container("immich_server").unwrap();
        c.backup_status(3).unwrap();
        c.backup_task(3).unwrap();
        c.backup_target(3).unwrap();
        c.storage().unwrap();
        c.system_utilization().unwrap();
        c.container_resources().unwrap();
        c.networks().unwrap();
        for m in mocks {
            m.assert_calls(1);
        }
    }

    #[test]
    fn api_info_lists_every_api() {
        let server = MockServer::start();
        server.mock(|w, t| {
            w.path("/webapi/query.cgi").query_param("query", "all");
            t.json_body(json!({"success": true, "data": {
                "SYNO.Docker.Container": {"path": "entry.cgi"}, "SYNO.API.Auth": {"path": "entry.cgi"}}}));
        });
        let apis = client(&server).api_info().unwrap();
        assert!(apis.contains("SYNO.Docker.Container"));
        assert!(!apis.contains("SYNO.Backup.Task"));
    }

    #[test]
    fn transport_errors_never_show_the_password_or_sid() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let dir = tempfile::tempdir().unwrap();
        SessionStore::new(dir.path()).store("nas", "u", "SECRETSID");
        let c = DsmClient::with_base_url(
            format!("http://127.0.0.1:{port}"),
            "nas".into(),
            "u".into(),
            Password::Literal("s3cr3t-pw".into()),
            6,
            false,
        )
        .unwrap();
        let e = c.system_info().unwrap_err().to_string();
        assert!(e.starts_with("synology request: "), "{e}");
        assert!(!e.contains("s3cr3t-pw") && !e.contains("passwd"), "{e}");
        let e = c
            .with_session_store(SessionStore::new(dir.path()))
            .system_info()
            .unwrap_err()
            .to_string();
        assert!(!e.contains("SECRETSID") && !e.contains("_sid"), "{e}");
    }

    #[test]
    fn login_check_reports_the_code() {
        let server = MockServer::start();
        mock_auth(&server);
        mock_login_error(&server, 400);
        assert_eq!(client(&server).login_check(), Err(AccessError::Login(400)));
    }
}
