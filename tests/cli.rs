use std::process::{Command, Output};

fn synoctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_synoctl"))
        .args(args)
        .env_remove("SYNOCTL_CONFIG")
        .output()
        .expect("run synoctl")
}

#[test]
fn version_flag_prints_crate_version() {
    let out = synoctl(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("synoctl {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_command_is_usage_error() {
    assert_eq!(synoctl(&["bogus"]).status.code(), Some(2));
}

#[cfg(feature = "fixtures")]
mod with_fixtures {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn run_in(dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_synoctl"))
            .arg("--fixtures")
            .arg(dir)
            .args(args)
            .env("TZ", "UTC")
            .env("SYNOCTL_NOW", "2026-04-25T12:00:00Z")
            .env_remove("NO_COLOR")
            .output()
            .unwrap()
    }

    fn run(args: &[&str]) -> Output {
        run_in(&fixtures(), args)
    }

    fn stdout(o: &Output) -> String {
        String::from_utf8(o.stdout.clone()).unwrap()
    }

    fn stderr(o: &Output) -> String {
        String::from_utf8(o.stderr.clone()).unwrap()
    }

    /// Drops `ESC [ … m` sequences.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    const TABLE_COMMANDS: &[&[&str]] = &[
        &["status"],
        &["info"],
        &["utilization"],
        &["volumes", "list"],
        &["volumes", "list", "--wide"],
        &["volumes", "get", "volume_1"],
        &["backups", "list"],
        &["backups", "list", "--wide"],
        &["backups", "get", "3"],
        &["docker", "containers", "list"],
        &["docker", "containers", "list", "--wide"],
        &["docker", "containers", "list", "--all"],
        &["docker", "containers", "get", "immich_server"],
        &["docker", "images", "list"],
        &["docker", "images", "get", "aabbccddeeff"],
        &["docker", "networks", "list"],
        &["docker", "networks", "get", "host"],
        &["docker", "updates", "list"],
        &["docker", "updates", "list", "--all"],
        &["docker", "updates", "get", "immich_server"],
        &["docker", "updates", "check"],
    ];

    #[test]
    fn every_command_runs_against_fixtures() {
        for args in TABLE_COMMANDS {
            let o = run(args);
            assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
            assert!(!stdout(&o).is_empty(), "{args:?}");
        }
    }

    fn walk(v: &serde_json::Value, path: &str) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    assert!(k != "device" && k != "uri", "{path}.{k}");
                    walk(v, &format!("{path}.{k}"));
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, path)),
            _ => {}
        }
    }

    #[test]
    fn json_output_parses_and_has_no_device_or_uri() {
        for args in TABLE_COMMANDS {
            let o = run(&[args, &["-o", "json"][..]].concat());
            assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stderr(&o));
            let v: serde_json::Value =
                serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}"));
            walk(&v, &format!("{args:?}"));
        }
    }

    #[test]
    fn ids_output() {
        assert_eq!(
            stdout(&run(&[
                "docker",
                "containers",
                "list",
                "--all",
                "-o",
                "ids"
            ])),
            "immich_server\nimmich_machine_learning\nprometheus\n"
        );
        assert_eq!(stdout(&run(&["backups", "list", "-o", "ids"])), "3\n4\n");
        assert_eq!(
            stdout(&run(&["volumes", "list", "-o", "ids"])),
            "volume_1\n"
        );
        assert_eq!(
            stdout(&run(&["docker", "networks", "list", "-o", "ids"]))
                .lines()
                .count(),
            4
        );
    }

    #[test]
    fn ids_on_a_non_list_is_an_error() {
        let o = run(&["info", "-o", "ids"]);
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(
            stderr(&o),
            "Error: -o ids is only supported by list commands\n"
        );
    }

    #[test]
    fn not_found_exits_1_with_message() {
        let o = run(&["docker", "containers", "get", "nope"]);
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(stderr(&o), "Error: Container not found: nope\n");
        let o = run(&["backups", "get", "9"]);
        assert_eq!(stderr(&o), "Error: Backup task not found: 9\n");
    }

    #[test]
    fn container_actions_print_past_tense() {
        for (verb, past) in [
            ("start", "Started"),
            ("stop", "Stopped"),
            ("restart", "Restarted"),
        ] {
            let o = run(&["docker", "containers", verb, "prometheus"]);
            assert_eq!(o.status.code(), Some(0));
            assert_eq!(stdout(&o), format!("{past} prometheus\n"));
        }
        let o = run(&[
            "-o",
            "json",
            "docker",
            "containers",
            "restart",
            "prometheus",
        ]);
        assert_eq!(stdout(&o), "");
        let o = run(&["docker", "containers", "stop", "nope"]);
        assert_eq!(o.status.code(), Some(1));
    }

    #[test]
    fn status_exits_0_on_warnings() {
        let o = run(&["status"]);
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
        assert!(stdout(&o).contains("CONTAINERS  warn"), "{}", stdout(&o));
    }

    #[test]
    fn backup_warnings_go_to_stderr() {
        let o = run(&["backups", "list"]);
        assert!(
            stderr(&o).contains("warning: backup task Backup to REMOTE: status unavailable"),
            "{}",
            stderr(&o)
        );
    }

    #[test]
    fn container_status_filter() {
        assert_eq!(
            stdout(&run(&[
                "docker",
                "containers",
                "list",
                "--status",
                "stopped",
                "-o",
                "ids"
            ])),
            "prometheus\n"
        );
    }

    #[test]
    fn containers_list_shows_running_by_default() {
        assert_eq!(
            stdout(&run(&["docker", "containers", "list", "-o", "ids"])),
            "immich_server\nimmich_machine_learning\n"
        );
        let o = run(&[
            "docker",
            "containers",
            "list",
            "--all",
            "--status",
            "stopped",
        ]);
        assert_eq!(o.status.code(), Some(2), "--all conflicts with --status");
    }

    #[test]
    fn updates_list_shows_available_by_default() {
        for cmd in ["list", "check"] {
            assert_eq!(stdout(&run(&["docker", "updates", cmd, "-o", "ids"])), "");
            assert_eq!(
                stdout(&run(&["docker", "updates", cmd, "--all", "-o", "ids"])),
                "immich_server\nimmich_machine_learning\nprometheus\n"
            );
        }
        let o = run(&["docker", "updates", "list", "--status", "up-to-date"]);
        assert_eq!(o.status.code(), Some(0), "kebab-case alias");
    }

    #[test]
    fn volumes_and_backups_filters() {
        assert_eq!(
            stdout(&run(&[
                "volumes", "list", "--status", "crashed", "-o", "ids"
            ])),
            ""
        );
        assert_eq!(
            stdout(&run(&[
                "backups", "list", "--result", "success", "-o", "ids"
            ])),
            "3\n"
        );
        assert_eq!(
            stdout(&run(&[
                "backups", "list", "--status", "idle", "--result", "unknown", "-o", "ids"
            ])),
            "4\n"
        );
    }

    #[test]
    fn updates_status_filter_and_source_warning() {
        let o = run(&[
            "docker", "updates", "list", "--status", "unknown", "-o", "ids",
        ]);
        assert_eq!(stdout(&o), "prometheus\n");
        assert!(
            stderr(&o).contains("warning: no release source for prom/prometheus"),
            "{}",
            stderr(&o)
        );
    }

    #[test]
    fn color_always_paints_without_moving_columns() {
        for args in TABLE_COMMANDS {
            let plain = stdout(&run(&[args, &["--color", "never"][..]].concat()));
            let painted = stdout(&run(&[args, &["--color", "always"][..]].concat()));
            assert_eq!(strip_ansi(&painted), plain, "{args:?}");
        }
        assert!(stdout(&run(&["status", "--color", "always"])).contains("\x1b["));
    }

    #[test]
    fn docker_without_container_manager() {
        let dir = tempfile::tempdir().unwrap();
        for e in std::fs::read_dir(fixtures()).unwrap() {
            let e = e.unwrap();
            if e.file_name() != "dsm-containers.json" {
                std::fs::copy(e.path(), dir.path().join(e.file_name())).unwrap();
            }
        }
        let o = run_in(dir.path(), &["docker", "images", "list"]);
        assert_eq!(o.status.code(), Some(1));
        assert_eq!(
            stderr(&o),
            "Error: Container Manager is not installed on fixture\n"
        );
        let out = stdout(&run_in(dir.path(), &["status"]));
        assert!(
            !out.contains("CONTAINERS") && !out.contains("UPDATES"),
            "{out}"
        );
    }
}

#[test]
fn missing_config_suggests_init() {
    let home = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_synoctl"))
        .arg("info")
        .env_remove("SYNOCTL_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("SYNO_HOST")
        .env_remove("SYNO_USERNAME")
        .env_remove("SYNO_PASSWORD")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        format!(
            "Error: config: no config file at {}; run `synoctl config init`\n",
            home.path().join(".config/synoctl/config.toml").display()
        )
    );
}
