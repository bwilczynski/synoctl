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
