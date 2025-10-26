use clap::Parser;
use fs4::fs_std::FileExt;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// A program that wraps a command, and ensures that only one instance of the
/// command is running at a time.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The name of the tmux window to use.
    #[arg(long = "tmux_window_name")]
    tmux_window_name: Option<String>,

    /// The path to the lockfile.
    #[arg(long = "lockfile")]
    lockfile: Option<String>,

    /// The lock_timeout in seconds.
    #[arg(long = "lock_timeout")]
    lock_timeout: Option<u64>,

    /// The command_timeout in seconds.
    #[arg(long = "command_timeout")]
    command_timeout: Option<u64>,

    /// The directory to run the command in.
    #[arg(long = "directory")]
    directory: Option<String>,

    /// The command to run.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
    // TODO: add support for shell?
}

#[allow(dead_code)]
fn lock_file(lock_filename: &Path, lock_timeout: Duration) -> Result<File, String> {
    let start = Instant::now();
    loop {
        let file = File::create(lock_filename).map_err(|e| e.to_string())?;
        match file.try_lock_exclusive() {
            Ok(true) => return Ok(file),
            Ok(false) => {
                if start.elapsed() > lock_timeout {
                    return Err(format!(
                        "Timeout waiting for lockfile after {lock_timeout:?} seconds"
                    )
                    .to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // TODO: how do I test this?
            Err(error_message) => {
                return Err(error_message.to_string());
            }
        }
    }
}

#[allow(dead_code)]
fn run_command(
    command: &[String],
    timeout: Option<Duration>,
    directory: Option<&String>,
) -> Result<i32, String> {
    let mut child_command = Command::new(&command[0]);
    child_command.args(&command[1..]);
    if let Some(dir) = directory {
        child_command.current_dir(dir);
    }
    child_command.process_group(0);

    // TODO: record the child pgid somewhere and kill it on receipt of SIGINT.
    let mut child = child_command.spawn().map_err(|e| e.to_string())?;

    let exit_status = match timeout {
        Some(duration) => match child.wait_timeout(duration).map_err(|e| e.to_string())? {
            Some(status) => Ok(status),
            None => {
                let pgid = Pid::from_raw(child.id() as i32);
                // TODO: send SIGINT first to give time for a graceful exit, then send SIGKILL
                // after 1 second.
                killpg(pgid, Signal::SIGKILL).map_err(|e| e.to_string())?;
                child.wait().map_err(|e| e.to_string())?;
                Err(format!("Command timed out after {duration:?}"))
            }
        },
        None => child.wait().map_err(|e| e.to_string()),
    }?;

    exit_status
        .code()
        .ok_or_else(|| "Command terminated by signal".to_string())
}

fn realmain(args: Args) {
    println!("tmux_window_name: {:?}", args.tmux_window_name);
    println!("lockfile: {:?}", args.lockfile);
    println!("lock_timeout: {:?}", args.lock_timeout);
    println!("command_timeout: {:?}", args.command_timeout);
    println!("directory: {:?}", args.directory);
    println!("command: {:?}", args.command);
}

fn main() {
    realmain(Args::parse());
}

#[cfg(test)]
mod realmain {
    use super::*;

    #[test]
    fn not_really_a_test() {
        realmain(Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=foo",
            "--lockfile=bar",
            "--lock_timeout=100",
            "--command_timeout=100",
            "--directory=/tmp",
            "echo",
            "foo",
        ]));
    }
}

#[cfg(test)]
mod run_command {
    use super::*;
    use tempfile;

    #[test]
    fn test_run_command_success() {
        let command = vec!["echo".to_string(), "foo".to_string()];
        let result = run_command(&command, None, None);
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_run_command_timeout() {
        let command = vec!["sleep".to_string(), "2".to_string()];
        let result = run_command(&command, Some(Duration::from_secs(1)), None);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Command timed out after 1s");
    }

    #[test]
    fn test_run_command_success_with_timeout() {
        let command = vec!["sleep".to_string(), "1".to_string()];
        let result = run_command(&command, Some(Duration::from_secs(2)), None);
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_run_command_fail() {
        let command = vec!["false".to_string()];
        let result = run_command(&command, None, None);
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_run_command_in_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("foo.txt");
        File::create(file_path).unwrap();

        let command = vec!["test".to_string(), "-f".to_string(), "foo.txt".to_string()];
        let result = run_command(
            &command,
            None,
            Some(&temp_dir.path().to_str().unwrap().to_string()),
        );
        assert_eq!(result.unwrap(), 0);

        let result_fail = run_command(&command, None, None);
        assert_eq!(result_fail.unwrap(), 1);
    }
}

#[cfg(test)]
mod lock_file {
    use super::*;
    use std::env;
    use std::thread;

    #[test]
    fn test_lock_file() {
        let mut temp_file = env::temp_dir();
        temp_file.push("test.lock");

        let _lock = lock_file(&temp_file, Duration::from_secs(1)).unwrap();

        let lock_result = thread::spawn(move || lock_file(&temp_file, Duration::from_secs(1)))
            .join()
            .unwrap();

        assert!(lock_result.is_err());
    }
}
