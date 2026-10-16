//! `config init`: gathers host and account from flags, env and prompts,
//! then writes config.toml. The caller runs `config check` afterwards.

use std::path::Path;

use super::prompt::Prompter;
use crate::cli::InitArgs;
use crate::config::{self, FileConfig, Password};
use crate::dsm::client::{DsmClient, TransportError};
use crate::error::{Error, Result};

pub fn run(
    args: &InitArgs,
    path: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    p: &mut dyn Prompter,
) -> Result<()> {
    run_with(args, path, env, p, "https")
}

/// `scheme` is `http` only in tests, against a plain-HTTP mock NAS.
pub(crate) fn run_with(
    args: &InitArgs,
    path: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    p: &mut dyn Prompter,
    scheme: &str,
) -> Result<()> {
    if path.exists() && !args.force {
        return Err(Error::Config(format!(
            "{} already exists; pass --force to overwrite",
            path.display()
        )));
    }
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let host = match &args.host {
        Some(h) => config::normalize_host(h)?,
        None => ask_host(p)?,
    };
    let username = match &args.username {
        Some(u) => u.clone(),
        None => {
            require(p, "username", "--username")?;
            ask_nonempty(p, "Username")?
        }
    };
    let password = match (&args.password_command, &args.password) {
        (Some(cmd), _) => Password::Command(cmd.clone()),
        (None, Some(pw)) => Password::Literal(pw.clone()),
        (None, None) => match var("SYNO_PASSWORD") {
            Some(pw) => Password::Literal(pw),
            None => {
                require(
                    p,
                    "password",
                    "--password, --password-command or SYNO_PASSWORD",
                )?;
                ask_password(p)?
            }
        },
    };
    let (password, password_command) = match password {
        Password::Literal(pw) => (Some(pw), None),
        Password::Command(cmd) => (None, Some(cmd)),
    };
    let mut insecure_tls = args.insecure_tls;
    if !insecure_tls && !args.no_check {
        insecure_tls = offer_insecure(&host, scheme, p)?;
    }
    let file = FileConfig {
        host: Some(host),
        username: Some(username),
        password,
        password_command,
        insecure_tls: insecure_tls.then_some(true),
        ..Default::default()
    };
    config::write(path, &file)?;
    p.note(&format!("wrote {}", path.display()));
    Ok(())
}

/// Errors out instead of prompting when nobody is there to answer.
fn require(p: &dyn Prompter, field: &str, flags: &str) -> Result<()> {
    if p.interactive() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "{field} is required when not running interactively (pass {flags})"
        )))
    }
}

fn ask_host(p: &mut dyn Prompter) -> Result<String> {
    require(p, "host", "--host")?;
    loop {
        let raw = p.line("NAS address (host[:port], e.g. 192.168.1.10:5001)", None)?;
        match config::normalize_host(&raw) {
            Ok(host) if !host.is_empty() => return Ok(host),
            Ok(_) => p.note("host is required"),
            Err(e) => p.note(&e.to_string()),
        }
    }
}

fn ask_nonempty(p: &mut dyn Prompter, label: &str) -> Result<String> {
    loop {
        let answer = p.line(label, None)?;
        if !answer.is_empty() {
            return Ok(answer);
        }
    }
}

/// A password manager command, or else the password itself.
fn ask_password(p: &mut dyn Prompter) -> Result<Password> {
    let cmd = p.line(
        "Password command, e.g. `pass show nas/synoctl` (empty to type the password)",
        None,
    )?;
    if cmd.is_empty() {
        ask_secret(p, "Password").map(Password::Literal)
    } else {
        Ok(Password::Command(cmd))
    }
}

fn ask_secret(p: &mut dyn Prompter, label: &str) -> Result<String> {
    loop {
        let answer = p.read_secret(&format!("{label}: "))?;
        if !answer.is_empty() {
            return Ok(answer);
        }
    }
}

/// Asks about `insecure_tls` only once the certificate has actually failed.
/// Other connection failures are left for the final check to explain.
fn offer_insecure(host: &str, scheme: &str, p: &mut dyn Prompter) -> Result<bool> {
    let client = DsmClient::with_base_url(
        format!("{scheme}://{host}"),
        host.to_string(),
        String::new(),
        Password::Literal(String::new()),
        6,
        false,
    )?
    .with_timeout(std::time::Duration::from_secs(10));
    let Err(TransportError::Certificate(reason)) = client.handshake() else {
        return Ok(false);
    };
    if !p.interactive() {
        return Ok(false);
    }
    p.confirm(
        &format!(
            "The NAS's certificate is not trusted ({reason}). Accept it anyway (insecure_tls)?"
        ),
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use super::*;

    /// Answers prompts from a list; records what was asked and noted.
    struct Script {
        interactive: bool,
        answers: VecDeque<&'static str>,
        prompts: Vec<String>,
        notes: Vec<String>,
    }

    impl Script {
        fn new(interactive: bool, answers: &[&'static str]) -> Self {
            Self {
                interactive,
                answers: answers.iter().copied().collect(),
                prompts: Vec::new(),
                notes: Vec::new(),
            }
        }
    }

    impl Prompter for Script {
        fn interactive(&self) -> bool {
            self.interactive
        }
        fn read_line(&mut self, prompt: &str) -> Result<String> {
            self.prompts.push(prompt.to_string());
            self.answers
                .pop_front()
                .map(String::from)
                .ok_or_else(|| Error::Other("config init: aborted, nothing written".into()))
        }
        fn read_secret(&mut self, prompt: &str) -> Result<String> {
            self.read_line(prompt)
        }
        fn note(&mut self, msg: &str) {
            self.notes.push(msg.to_string());
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn target(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("synoctl/config.toml")
    }

    #[test]
    fn flags_only_without_check() {
        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            host: Some("https://192.168.1.10:5001/".into()),
            username: Some("synoctl".into()),
            password: Some("pw".into()),
            insecure_tls: true,
            no_check: true,
            ..Default::default()
        };
        let mut p = Script::new(false, &[]);
        run(&args, &target(&dir), &no_env, &mut p).unwrap();
        assert_eq!(
            std::fs::read_to_string(target(&dir)).unwrap(),
            "host = \"192.168.1.10:5001\"\nusername = \"synoctl\"\npassword = \"pw\"\ninsecure_tls = true\n"
        );
        assert!(p.prompts.is_empty());
        assert_eq!(p.notes, vec![format!("wrote {}", target(&dir).display())]);
        #[cfg(unix)]
        assert_eq!(crate::config::file_mode(&target(&dir)), Some(0o600));
    }

    #[test]
    fn password_from_env() {
        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            host: Some("nas".into()),
            username: Some("u".into()),
            no_check: true,
            ..Default::default()
        };
        let env = |k: &str| (k == "SYNO_PASSWORD").then(|| "secret".to_string());
        run(&args, &target(&dir), &env, &mut Script::new(false, &[])).unwrap();
        assert!(
            std::fs::read_to_string(target(&dir))
                .unwrap()
                .contains("password = \"secret\"")
        );
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            host: Some("h".into()),
            username: Some("u".into()),
            password: Some("p".into()),
            no_check: true,
            ..Default::default()
        };
        run(&args, &target(&dir), &no_env, &mut Script::new(false, &[])).unwrap();
        let e = run(&args, &target(&dir), &no_env, &mut Script::new(false, &[])).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!(
                "config: {} already exists; pass --force to overwrite",
                target(&dir).display()
            )
        );
        let forced = InitArgs {
            force: true,
            ..args
        };
        run(
            &forced,
            &target(&dir),
            &no_env,
            &mut Script::new(false, &[]),
        )
        .unwrap();
    }

    #[test]
    fn non_interactive_without_flags_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let e = run(
            &InitArgs::default(),
            &target(&dir),
            &no_env,
            &mut Script::new(false, &[]),
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "config: host is required when not running interactively (pass --host)"
        );
        let args = InitArgs {
            host: Some("nas".into()),
            no_check: true,
            ..Default::default()
        };
        let e = run(&args, &target(&dir), &no_env, &mut Script::new(false, &[])).unwrap_err();
        assert!(e.to_string().contains("--username"), "{e}");
    }

    #[test]
    fn interactive_answers() {
        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            no_check: true,
            ..Default::default()
        };
        let mut p = Script::new(true, &["", "nas.lan:5001", "admin", "", "", "pw"]);
        run(&args, &target(&dir), &no_env, &mut p).unwrap();
        assert_eq!(
            std::fs::read_to_string(target(&dir)).unwrap(),
            "host = \"nas.lan:5001\"\nusername = \"admin\"\npassword = \"pw\"\n"
        );
        assert_eq!(
            p.prompts,
            [
                "NAS address (host[:port], e.g. 192.168.1.10:5001): ",
                "NAS address (host[:port], e.g. 192.168.1.10:5001): ",
                "Username: ",
                PASSWORD_COMMAND,
                "Password: ",
                "Password: "
            ]
        );
    }

    const PASSWORD_COMMAND: &str =
        "Password command, e.g. `pass show nas/synoctl` (empty to type the password): ";

    #[test]
    fn password_command_is_written_instead_of_a_password() {
        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            host: Some("nas".into()),
            username: Some("u".into()),
            password_command: Some("pass show nas/synoctl".into()),
            no_check: true,
            ..Default::default()
        };
        let env = |k: &str| (k == "SYNO_PASSWORD").then(|| "secret".to_string());
        run(&args, &target(&dir), &env, &mut Script::new(false, &[])).unwrap();
        assert_eq!(
            std::fs::read_to_string(target(&dir)).unwrap(),
            "host = \"nas\"\nusername = \"u\"\npassword_command = \"pass show nas/synoctl\"\n"
        );

        let dir = tempfile::tempdir().unwrap();
        let args = InitArgs {
            no_check: true,
            ..Default::default()
        };
        let mut p = Script::new(true, &["nas", "u", "pass show synoctl"]);
        run(&args, &target(&dir), &no_env, &mut p).unwrap();
        assert!(
            std::fs::read_to_string(target(&dir))
                .unwrap()
                .ends_with("password_command = \"pass show synoctl\"\n")
        );
        assert_eq!(p.prompts.last().map(String::as_str), Some(PASSWORD_COMMAND));
    }
}
