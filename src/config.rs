//! Config: ~/.config/synoctl/config.toml overlaid with SYNO_* env vars.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::SignedDuration;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const DEFAULT_AUTH_VERSION: u32 = 6;
const DEFAULT_CACHE_TTL_SECS: u64 = 15;
const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
pub struct Config {
    /// host[:port]; always reached over https.
    pub host: String,
    pub username: String,
    pub password: Password,
    pub insecure_tls: bool,
    /// The NAS's time zone, for DSM timestamps that carry no offset.
    pub timezone: TimeZone,
    /// `SYNO.API.Auth` version used to log in.
    pub auth_version: u32,
    /// How long DSM responses are reused; zero disables the cache.
    pub cache_ttl: Duration,
    pub updates: UpdatesConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatesConfig {
    /// How long release lookups are reused.
    pub check_interval: Duration,
    pub sources: Vec<SourceConfig>,
}

/// Where an image's releases are published.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// The image without its tag, e.g. `vaultwarden/server`.
    pub image: String,
    /// `owner/repo` on the release host.
    pub source: String,
    #[serde(default)]
    pub r#type: SourceType,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    #[default]
    Github,
    Codeberg,
}

/// Where the login password comes from.
#[derive(Clone, PartialEq, Eq)]
pub enum Password {
    /// From the config file or `SYNO_PASSWORD`.
    Literal(String),
    /// Printed by this shell command (`password_command`), e.g. a password
    /// manager's CLI. Run only when synoctl has to log in.
    Command(String),
}

/// Never shows a literal password.
impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Literal(_) => f.write_str("Literal(****)"),
            Self::Command(c) => f.debug_tuple("Command").field(c).finish(),
        }
    }
}

impl Password {
    /// The password: the literal, or the first line the command prints. The
    /// command gets the terminal's stdin and stderr, so a password manager
    /// can ask to be unlocked and its errors reach the user.
    pub fn resolve(&self) -> Result<String> {
        let cmd = match self {
            Self::Literal(p) => return Ok(p.clone()),
            Self::Command(c) => c,
        };
        let fail = |m: String| Error::Auth(format!("password_command `{cmd}`: {m}"));
        let out = shell(cmd)
            .stdin(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_err(|e| fail(format!("cannot run: {e}")))?;
        if !out.status.success() {
            return Err(fail(match out.status.code() {
                Some(code) => format!("exited with status {code}"),
                None => "killed by a signal".into(),
            }));
        }
        let text =
            String::from_utf8(out.stdout).map_err(|_| fail("printed invalid UTF-8".into()))?;
        let line = text.lines().next().unwrap_or_default();
        if line.is_empty() {
            return Err(fail("printed nothing".into()));
        }
        Ok(line.to_string())
    }
}

#[cfg(unix)]
fn shell(cmd: &str) -> std::process::Command {
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

#[cfg(not(unix))]
fn shell(cmd: &str) -> std::process::Command {
    let mut c = std::process::Command::new("cmd");
    c.arg("/C").arg(cmd);
    c
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Non-fatal problems to print on stderr.
    pub warnings: Vec<String>,
    /// The file itself holds a literal secret, so its permissions matter.
    pub secret_in_file: bool,
}

/// The keys of config.toml. `config init` writes only the ones it sets.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insecure_tls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_ttl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updates: Option<FileUpdates>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileUpdates {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_interval: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceConfig>,
}

/// `--config` > `SYNOCTL_CONFIG` > `$XDG_CONFIG_HOME/synoctl/config.toml` >
/// `$HOME/.config/synoctl/config.toml`.
pub fn resolve_path(flag: Option<&Path>, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(p) = var("SYNOCTL_CONFIG") {
        return PathBuf::from(p);
    }
    let base = var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("synoctl").join("config.toml")
}

/// `$XDG_CACHE_HOME/synoctl` > `$HOME/.cache/synoctl`.
pub fn resolve_cache_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let base = var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("synoctl")
}

/// Writes `file` to `path` owner-only, creating its directory.
pub fn write(path: &Path, file: &FileConfig) -> Result<()> {
    let body = toml::to_string(file).map_err(|e| Error::Config(format!("encode config: {e}")))?;
    let fail = |e: std::io::Error| Error::Config(format!("cannot write {}: {e}", path.display()));
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        crate::private_fs::create_dir(dir).map_err(fail)?;
    }
    crate::private_fs::replace(path, &body).map_err(fail)
}

/// Reads `path` (a missing file is fine unless the user named it explicitly),
/// applies env overrides and validates.
pub fn load(path: &Path, explicit: bool, env: &dyn Fn(&str) -> Option<String>) -> Result<Loaded> {
    let mut warnings = Vec::new();
    let mut no_file = false;
    let file = match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str::<FileConfig>(&text).map_err(|e| parse_error(path, &text, &e))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
            no_file = true;
            FileConfig::default()
        }
        Err(e) => {
            return Err(Error::Config(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let secret_in_file = holds_secret(&file);
    if !no_file {
        warnings.extend(permission_warning(path, secret_in_file));
    }
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let pick =
        |key: &str, from_file: Option<String>| var(key).or(from_file).filter(|v| !v.is_empty());

    let host = normalize_host(&pick("SYNO_HOST", file.host).unwrap_or_default())?;
    if host.is_empty() {
        if no_file {
            return Err(Error::Config(format!(
                "no config file at {}; run `synoctl config init`",
                path.display()
            )));
        }
        return Err(missing("host", "SYNO_HOST", path));
    }
    let username = pick("SYNO_USERNAME", file.username)
        .ok_or_else(|| missing("username", "SYNO_USERNAME", path))?;
    let password_command = file.password_command.filter(|c| !c.trim().is_empty());
    if file.password.is_some() && password_command.is_some() {
        return Err(Error::Config(format!(
            "{}: set either password or password_command, not both",
            path.display()
        )));
    }
    let password = match (pick("SYNO_PASSWORD", file.password), password_command) {
        (Some(p), _) => Password::Literal(p),
        (None, Some(c)) => Password::Command(c),
        (None, None) => {
            return Err(Error::Config(format!(
                "password is required (set password or password_command in {}, or SYNO_PASSWORD)",
                path.display()
            )));
        }
    };

    let insecure_tls = match var("SYNO_INSECURE_TLS") {
        Some(v) => parse_bool("SYNO_INSECURE_TLS", &v)?,
        None => file.insecure_tls.unwrap_or(false),
    };
    let timezone = match pick("SYNO_TIMEZONE", file.timezone) {
        Some(name) => TimeZone::get(&name)
            .map_err(|e| Error::Config(format!("timezone: unknown time zone {name:?} ({e})")))?,
        None => TimeZone::system(),
    };
    let auth_version = match var("SYNO_AUTH_VERSION") {
        Some(v) => parse_number("SYNO_AUTH_VERSION", "an integer", &v)?,
        None => file.auth_version.unwrap_or(DEFAULT_AUTH_VERSION),
    };
    let cache_ttl = match var("SYNO_CACHE_TTL") {
        Some(v) => parse_number("SYNO_CACHE_TTL", "a number of seconds", &v)?,
        None => file.cache_ttl.unwrap_or(DEFAULT_CACHE_TTL_SECS),
    };
    let file_updates = file.updates.unwrap_or_default();
    let check_interval = match &file_updates.check_interval {
        Some(s) => parse_interval(s)?,
        None => DEFAULT_CHECK_INTERVAL,
    };

    Ok(Loaded {
        config: Config {
            host,
            username,
            password,
            insecure_tls,
            timezone,
            auth_version,
            cache_ttl: Duration::from_secs(cache_ttl),
            updates: UpdatesConfig {
                check_interval,
                sources: file_updates.sources,
            },
        },
        warnings,
        secret_in_file,
    })
}

fn missing(key: &str, env: &str, path: &Path) -> Error {
    Error::Config(format!(
        "{key} is required (set {key} in {} or {env})",
        path.display()
    ))
}

/// `1h`, `30m`, `6h`: jiff's friendly duration format.
fn parse_interval(s: &str) -> Result<Duration> {
    let fail = |m: String| Error::Config(format!("updates.check_interval: {m}"));
    let d: SignedDuration = s.parse().map_err(|e| {
        fail(format!(
            "expected a duration such as 1h or 30m, got {s:?} ({e})"
        ))
    })?;
    if d <= SignedDuration::ZERO {
        return Err(fail("must be positive".into()));
    }
    Duration::try_from(d).map_err(|e| fail(e.to_string()))
}

fn parse_number<T: std::str::FromStr>(name: &str, what: &str, v: &str) -> Result<T> {
    v.parse()
        .map_err(|_| Error::Config(format!("{name}: expected {what}, got {v:?}")))
}

/// One line naming the position, never toml's snippet of the file: the
/// snippet can show a secret, and `config check` output gets pasted around.
fn parse_error(path: &Path, text: &str, e: &toml::de::Error) -> Error {
    let at = e.span().map_or(String::new(), |span| {
        let before = &text[..span.start.min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!(" at line {line}, column {column}")
    });
    Error::Config(format!("{}: {}{at}", path.display(), e.message()))
}

/// Accepts `host`, `host:port`, `[v6]:port`, a bare v6 literal (bracketed
/// here, as URLs need) or `https://host[:port]/`.
pub fn normalize_host(raw: &str) -> Result<String> {
    let host = raw
        .trim()
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if host.contains("://") || host.contains('/') {
        return Err(Error::Config(format!(
            "host must be a hostname or host:port (the NAS is always reached over https), got {raw:?}"
        )));
    }
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        return Ok(format!("[{host}]"));
    }
    Ok(host.to_string())
}

fn parse_bool(name: &str, v: &str) -> Result<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(Error::Config(format!(
            "{name}: expected true or false, got {v:?}"
        ))),
    }
}

/// The env vars that override config keys, in the order `config check` lists them.
const ENV_VARS: [&str; 7] = [
    "SYNO_HOST",
    "SYNO_USERNAME",
    "SYNO_PASSWORD",
    "SYNO_INSECURE_TLS",
    "SYNO_TIMEZONE",
    "SYNO_AUTH_VERSION",
    "SYNO_CACHE_TTL",
];

/// `NAME=value` for every non-empty override, the password masked.
pub fn env_overrides(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    ENV_VARS
        .iter()
        .filter_map(|&name| {
            let value = env(name).filter(|v| !v.is_empty())?;
            let shown = match name {
                "SYNO_PASSWORD" => "****".to_string(),
                _ => value,
            };
            Some(format!("{name}={shown}"))
        })
        .collect()
}

/// Permission bits of `path`; `None` off Unix or when it cannot be read.
#[cfg(unix)]
pub fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(std::fs::metadata(path).ok()?.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
pub fn file_mode(_path: &Path) -> Option<u32> {
    None
}

/// Whether the file holds a literal password. A file that names only a
/// `password_command` is safe to leave readable, e.g. symlinked from a
/// dotfiles checkout.
fn holds_secret(file: &FileConfig) -> bool {
    file.password.as_deref().is_some_and(|p| !p.is_empty())
}

/// What is wrong with a config file's `mode`, if anything. Others writing it
/// is always a problem: they could point a secret command, or the host, at
/// their own. Others reading it is one only when it holds a literal secret.
pub fn mode_problem(mode: u32, secret_in_file: bool) -> Option<&'static str> {
    if mode & 0o022 != 0 {
        Some("writable by other users")
    } else if secret_in_file && mode & 0o044 != 0 {
        Some("readable by other users")
    } else {
        None
    }
}

fn permission_warning(path: &Path, secret_in_file: bool) -> Option<String> {
    let problem = mode_problem(file_mode(path)?, secret_in_file)?;
    Some(format!(
        "{} is {problem}; run `chmod 600 {}`",
        path.display(),
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn write_file(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("config.toml");
        std::fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        p
    }

    const MINIMAL: &str = "host = \"nas.lan:5001\"\nusername = \"u\"\npassword = \"p\"\n";

    #[test]
    fn resolve_path_precedence() {
        let flag = Path::new("/x/flag.toml");
        let env = env_of(&[
            ("SYNOCTL_CONFIG", "/x/env.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ]);
        assert_eq!(resolve_path(Some(flag), &env), flag);
        assert_eq!(resolve_path(None, &env), Path::new("/x/env.toml"));
        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(
            resolve_path(None, &env),
            Path::new("/xdg/synoctl/config.toml")
        );
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            resolve_path(None, &env),
            Path::new("/home/u/.config/synoctl/config.toml")
        );
    }

    #[test]
    fn resolve_cache_dir_precedence() {
        let env = env_of(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/home/u")]);
        assert_eq!(resolve_cache_dir(&env), Path::new("/c/synoctl"));
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(resolve_cache_dir(&env), Path::new("/home/u/.cache/synoctl"));
    }

    #[test]
    fn minimal_file_gets_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, MINIMAL);
        let c = load(&p, true, &env_of(&[])).unwrap().config;
        assert_eq!(c.host, "nas.lan:5001");
        assert_eq!(c.username, "u");
        assert_eq!(c.password, Password::Literal("p".into()));
        assert_eq!(c.auth_version, 6);
        assert_eq!(c.cache_ttl, Duration::from_secs(15));
        assert!(!c.insecure_tls);
        assert_eq!(c.updates.check_interval, Duration::from_secs(3600));
        assert!(c.updates.sources.is_empty());
    }

    #[test]
    fn env_overrides_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, "host = \"a\"\nusername = \"u\"\npassword = \"p\"\n");
        let env = env_of(&[
            ("SYNO_HOST", "b:5001"),
            ("SYNO_PASSWORD", "q"),
            ("SYNO_INSECURE_TLS", "true"),
            ("SYNO_TIMEZONE", "Europe/Warsaw"),
            ("SYNO_AUTH_VERSION", "3"),
            ("SYNO_CACHE_TTL", "0"),
        ]);
        let c = load(&p, true, &env).unwrap().config;
        assert_eq!(
            (c.host.as_str(), c.insecure_tls, c.auth_version),
            ("b:5001", true, 3)
        );
        assert_eq!(c.password, Password::Literal("q".into()));
        assert_eq!(c.timezone.iana_name(), Some("Europe/Warsaw"));
        assert!(c.cache_ttl.is_zero());
    }

    #[test]
    fn missing_keys_name_key_and_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, "host = \"a\"\npassword = \"p\"\n");
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert_eq!(
            e,
            format!(
                "config: username is required (set username in {} or SYNO_USERNAME)",
                p.display()
            )
        );
        let p = write_file(&dir, "host = \"a\"\nusername = \"u\"\n");
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert!(
            e.contains("password is required")
                && e.contains("password_command")
                && e.contains("SYNO_PASSWORD"),
            "{e}"
        );
        let p = write_file(&dir, "username = \"u\"\npassword = \"p\"\n");
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert!(
            e.contains("host is required") && e.contains("SYNO_HOST"),
            "{e}"
        );
    }

    #[test]
    fn password_command_is_kept_unrun() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            &dir,
            "host = \"a\"\nusername = \"u\"\npassword_command = \"rbw get nas\"\n",
        );
        let c = load(&p, true, &env_of(&[])).unwrap().config;
        assert_eq!(c.password, Password::Command("rbw get nas".into()));
        let c = load(&p, true, &env_of(&[("SYNO_PASSWORD", "q")]))
            .unwrap()
            .config;
        assert_eq!(c.password, Password::Literal("q".into()));
    }

    #[test]
    fn password_and_password_command_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            &dir,
            &format!("{MINIMAL}password_command = \"rbw get nas\"\n"),
        );
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert!(
            e.ends_with("set either password or password_command, not both"),
            "{e}"
        );
    }

    #[test]
    fn literal_password_is_masked_in_debug() {
        assert_eq!(
            format!("{:?}", Password::Literal("secret".into())),
            "Literal(****)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn password_command_prints_the_first_line() {
        let pw = Password::Command("printf 'hunter2\\nmore\\n'".into());
        assert_eq!(pw.resolve().unwrap(), "hunter2");
        let pw = Password::Command("printf 'crlf\\r\\n'".into());
        assert_eq!(pw.resolve().unwrap(), "crlf");
    }

    #[cfg(unix)]
    #[test]
    fn password_command_failures_name_the_command() {
        let e = Password::Command("exit 3".into()).resolve().unwrap_err();
        assert!(matches!(e, Error::Auth(_)));
        assert_eq!(
            e.to_string(),
            "password_command `exit 3`: exited with status 3"
        );
        let e = Password::Command("true".into()).resolve().unwrap_err();
        assert_eq!(e.to_string(), "password_command `true`: printed nothing");
    }

    #[test]
    fn no_file_and_no_env_suggests_init() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let e = load(&p, false, &env_of(&[])).unwrap_err().to_string();
        assert_eq!(
            e,
            format!(
                "config: no config file at {}; run `synoctl config init`",
                p.display()
            )
        );
    }

    #[test]
    fn explicit_missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("none.toml");
        let env = env_of(&[
            ("SYNO_HOST", "nas"),
            ("SYNO_USERNAME", "u"),
            ("SYNO_PASSWORD", "p"),
        ]);
        let e = load(&p, true, &env).unwrap_err().to_string();
        assert!(
            e.starts_with(&format!("config: cannot read {}", p.display())),
            "{e}"
        );
    }

    #[test]
    fn env_alone_is_enough() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("missing.toml");
        let env = env_of(&[
            ("SYNO_HOST", "nas"),
            ("SYNO_USERNAME", "u"),
            ("SYNO_PASSWORD", "p"),
        ]);
        assert_eq!(load(&p, false, &env).unwrap().config.host, "nas");
    }

    #[test]
    fn invalid_timezone_and_interval_are_config_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, &format!("{MINIMAL}timezone = \"Mars/Base\"\n"));
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert!(
            e.starts_with("config: timezone: unknown time zone \"Mars/Base\""),
            "{e}"
        );
        let p = write_file(
            &dir,
            &format!("{MINIMAL}[updates]\ncheck_interval = \"soon\"\n"),
        );
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert!(e.starts_with("config: updates.check_interval: "), "{e}");
        let p = write_file(
            &dir,
            &format!("{MINIMAL}[updates]\ncheck_interval = \"0s\"\n"),
        );
        let e = load(&p, true, &env_of(&[])).unwrap_err().to_string();
        assert_eq!(e, "config: updates.check_interval: must be positive");
    }

    #[test]
    fn bad_env_values_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, MINIMAL);
        for (k, v) in [
            ("SYNO_INSECURE_TLS", "maybe"),
            ("SYNO_AUTH_VERSION", "x"),
            ("SYNO_CACHE_TTL", "-1"),
        ] {
            let e = load(&p, true, &env_of(&[(k, v)])).unwrap_err().to_string();
            assert!(e.contains(k), "{e}");
        }
    }

    #[test]
    fn update_sources_parse_with_github_default() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            &dir,
            &format!(
                "{MINIMAL}{}",
                concat!(
                    "[updates]\ncheck_interval = \"30m\"\n",
                    "[[updates.sources]]\nimage = \"vaultwarden/server\"\nsource = \"dani-garcia/vaultwarden\"\n",
                    "[[updates.sources]]\nimage = \"codeberg.org/forgejo/forgejo\"\nsource = \"forgejo/forgejo\"\ntype = \"codeberg\"\n"
                )
            ),
        );
        let u = load(&p, true, &env_of(&[])).unwrap().config.updates;
        assert_eq!(u.check_interval, Duration::from_secs(1800));
        assert_eq!(u.sources.len(), 2);
        assert_eq!(u.sources[0].image, "vaultwarden/server");
        assert_eq!(u.sources[0].r#type, SourceType::Github);
        assert_eq!(u.sources[1].r#type, SourceType::Codeberg);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, &format!("{MINIMAL}api_key = \"k\"\n"));
        assert!(load(&p, true, &env_of(&[])).is_err());
    }

    #[test]
    fn https_url_host_is_normalized_and_http_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n");
        let env = env_of(&[
            ("SYNO_HOST", "https://192.168.1.10:5001/"),
            ("SYNO_USERNAME", "u"),
            ("SYNO_PASSWORD", "p"),
        ]);
        assert_eq!(
            load(&p, false, &env).unwrap().config.host,
            "192.168.1.10:5001"
        );
        let env = env_of(&[
            ("SYNO_HOST", "http://192.168.1.10"),
            ("SYNO_USERNAME", "u"),
            ("SYNO_PASSWORD", "p"),
        ]);
        let e = load(&p, false, &env).unwrap_err().to_string();
        assert!(e.contains("host must be"), "{e}");
    }

    #[test]
    fn bare_ipv6_host_is_bracketed() {
        assert_eq!(normalize_host("fd00::1").unwrap(), "[fd00::1]");
        assert_eq!(normalize_host("[fd00::1]:5001").unwrap(), "[fd00::1]:5001");
        assert_eq!(normalize_host("nas.lan:5001").unwrap(), "nas.lan:5001");
    }

    #[test]
    fn parse_errors_do_not_echo_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, "host = \"h\"\npassword = \"supersecretpw\n");
        let err = load(&p, false, &env_of(&[])).unwrap_err().to_string();
        assert!(!err.contains("supersecret"), "{err}");
        assert!(!err.contains('\n'), "{err}");
        assert!(err.contains("line 2"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn group_readable_file_warns() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(&dir, MINIMAL);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let w = load(&p, true, &env_of(&[])).unwrap().warnings;
        assert_eq!(
            w,
            vec![format!(
                "{} is readable by other users; run `chmod 600 {}`",
                p.display(),
                p.display()
            )]
        );
    }

    #[cfg(unix)]
    #[test]
    fn readable_file_without_a_secret_does_not_warn() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            &dir,
            "host = \"h\"\nusername = \"u\"\npassword_command = \"pass show nas\"\n",
        );
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let l = load(&p, true, &env_of(&[])).unwrap();
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert!(!l.secret_in_file);
    }

    #[cfg(unix)]
    #[test]
    fn writable_file_warns_even_without_a_secret() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            &dir,
            "host = \"h\"\nusername = \"u\"\npassword_command = \"pass show nas\"\n",
        );
        for mode in [0o664, 0o666] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            let w = load(&p, true, &env_of(&[])).unwrap().warnings;
            assert_eq!(
                w,
                vec![format!(
                    "{} is writable by other users; run `chmod 600 {}`",
                    p.display(),
                    p.display()
                )],
                "{mode:o}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_creates_private_dirs_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("xdg/synoctl/config.toml");
        let file = FileConfig {
            host: Some("192.168.1.10:5001".into()),
            username: Some("u".into()),
            password: Some("p".into()),
            insecure_tls: Some(true),
            ..Default::default()
        };
        write(&p, &file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "host = \"192.168.1.10:5001\"\nusername = \"u\"\npassword = \"p\"\ninsecure_tls = true\n"
        );
        let l = load(&p, true, &env_of(&[])).unwrap();
        assert!(l.config.insecure_tls);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);
    }

    #[test]
    fn env_overrides_mask_password() {
        let env = env_of(&[("SYNO_HOST", "nas"), ("SYNO_PASSWORD", "secret")]);
        assert_eq!(
            env_overrides(&env),
            vec!["SYNO_HOST=nas", "SYNO_PASSWORD=****"]
        );
    }
}
