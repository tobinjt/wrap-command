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

#[test]
fn test_cli_run_command_not_found() {
    let output = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("nonexistent_command_12345")
        .output()
        .expect("failed to execute wrap-command");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(
        "Error: Failed to execute 'nonexistent_command_12345': No such file or directory"
    ));
}

#[test]
fn test_cli_run_command_directory_not_found() {
    let output = Command::new(env!("CARGO_BIN_EXE_wrap-command"))
        .arg("--directory=/nonexistent_dir_12345")
        .arg("echo")
        .arg("hello")
        .output()
        .expect("failed to execute wrap-command");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: Working directory '/nonexistent_dir_12345' does not exist"));
}
