use std::path::Path;
use std::process::{Command, Output};

/// Asks for bash completions of the last word in `words`, as the script from
/// `completions bash` would on TAB.
fn complete(words: &[&str], env: &[(&str, &Path)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_synoctl"))
        .env_clear()
        .env("COMPLETE", "bash")
        .env("_CLAP_IFS", "\n")
        .env("_CLAP_COMPLETE_INDEX", words.len().to_string())
        .envs(env.iter().copied())
        .arg("--")
        .arg("synoctl")
        .args(words)
        .output()
        .expect("run synoctl")
}

fn candidates(o: &Output) -> Vec<String> {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn script_registers_synoctl_for_each_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let o = Command::new(env!("CARGO_BIN_EXE_synoctl"))
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(o.status.success(), "{shell}");
        let script = String::from_utf8_lossy(&o.stdout);
        assert!(script.contains("COMPLETE"), "{shell}: {script}");
        assert!(script.contains("synoctl"), "{shell}: {script}");
    }
}

#[test]
fn subcommands_complete_without_a_nas() {
    assert_eq!(candidates(&complete(&["doc"], &[])), ["docker"]);
    assert_eq!(
        candidates(&complete(&["docker", "con"], &[])),
        ["containers"]
    );
}

#[test]
fn ids_complete_to_nothing_when_config_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let o = complete(
        &["volumes", "get", "v"],
        &[
            ("HOME", dir.path()),
            ("SYNOCTL_CONFIG", &dir.path().join("nope.toml")),
        ],
    );
    assert!(candidates(&o).is_empty());
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[cfg(feature = "fixtures")]
mod with_fixtures {
    use std::path::PathBuf;

    use super::{candidates, complete};

    fn ids(words: &[&str]) -> Vec<String> {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let o = complete(words, &[("SYNOCTL_FIXTURES", &fixtures)]);
        candidates(&o)
            .into_iter()
            .filter(|c| !c.starts_with('-'))
            .collect()
    }

    #[test]
    fn container_names_complete_for_every_container_command() {
        for verb in ["get", "start", "stop", "restart"] {
            assert_eq!(
                ids(&["docker", "containers", verb, "immich"]),
                ["immich_server", "immich_machine_learning"],
                "{verb}"
            );
        }
    }

    #[test]
    fn other_ids_complete() {
        assert_eq!(ids(&["volumes", "get", ""]), ["volume_1"]);
        assert_eq!(ids(&["backups", "get", ""]), ["3", "4"]);
        assert_eq!(ids(&["backups", "logs", ""]), ["3", "4"]);
        assert_eq!(ids(&["docker", "images", "get", "bb"]), ["bbccddeeff00"]);
        assert_eq!(
            ids(&["docker", "networks", "get", "im"]),
            ["immich_default"]
        );
        assert_eq!(ids(&["docker", "updates", "get", "pro"]), ["prometheus"]);
    }
}
