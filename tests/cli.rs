use std::process::Command;
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ysunethelper"))
}
#[test]
fn help_version_and_missing_targets() {
    for args in [
        vec!["--help"],
        vec!["-help"],
        vec!["--h"],
        vec!["-version"],
        vec!["version"],
        vec!["status", "-h"],
        vec!["switch", "-h"],
    ] {
        let out = binary().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    for args in [vec!["switch"], vec!["kick"], vec!["devices", "unexpected"]] {
        assert_eq!(binary().args(args).output().unwrap().status.code(), Some(2));
    }
}
#[test]
fn daemon_creates_template_and_exits_78_until_configured() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let out = binary()
        .arg("-config")
        .arg(&path)
        .arg("daemon")
        .output()
        .unwrap();
    assert!(out.status.success());
    let original = std::fs::read(&path).unwrap();
    let out = binary()
        .arg("--config")
        .arg(&path)
        .arg("daemon")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(78));
    assert_eq!(original, std::fs::read(&path).unwrap());
}
#[test]
fn global_config_flags_work_after_command_and_invalid_config_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.json");
    std::fs::write(&path, "{invalid").unwrap();
    let out = binary()
        .args(["status", "-config"])
        .arg(&path)
        .arg("-json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{invalid");
}
#[test]
fn malformed_daemon_config_is_a_configuration_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path,br#"{"username":"student","password":"pw","daemon":{"no_auth_period":{"enabled":true,"weekdays":[]}}}"#).unwrap();
    let out = binary()
        .arg("--config")
        .arg(path)
        .arg("daemon")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(78));
    assert_eq!(binary().output().unwrap().status.code(), Some(2));
}
#[test]
fn main_help_documents_all_commands_and_json_login_usage() {
    let output = binary().arg("--help").output().unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "status", "login", "logout", "switch", "tui", "devices", "kick", "daemon", "version",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
    assert!(help.contains("login [-u 账号] [-p 密码] [-s 运营商] [--json]"));
    for command in ["status", "login", "logout", "switch", "devices", "kick"] {
        let output = binary().args([command, "-h"]).output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("--json"));
    }
    let output = binary().args(["login", "-h"]).output().unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    for value in [
        "-u, --username",
        "-p, --password",
        "-s, --service",
        "campus/unicom/telecom/mobile",
    ] {
        assert!(help.contains(value));
    }
}
