use std::process::Command;

#[test]
fn test_cli_version() {
    let status = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("--version")
        .status()
        .expect("failed to execute wrap-command");
    assert!(status.success());
}

#[test]
fn test_cli_help() {
    let status = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("--help")
        .status()
        .expect("failed to execute wrap-command");
    assert!(status.success());
}

#[test]
fn test_cli_run_command_success() {
    let status = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("true")
        .status()
        .expect("failed to execute wrap-command");
    assert!(status.success());
}

#[test]
fn test_cli_run_command_failure() {
    let status = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("false")
        .status()
        .expect("failed to execute wrap-command");
    assert!(!status.success());
}
