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

    /// The lock_timeout in milliseconds.
    #[arg(long = "lock_timeout_ms")]
    lock_timeout_ms: Option<u64>,

    /// The command_timeout in milliseconds.
    #[arg(long = "command_timeout_ms")]
    command_timeout_ms: Option<u64>,

    /// The directory to run the command in.
    #[arg(long = "directory")]
    directory: Option<String>,

    /// Prepend `["sh", "-c"]` to the command.  Doesn't otherwise modify the command.
    #[arg(long = "shell")]
    shell: bool,

    /// The command to run.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

#[allow(dead_code)]
fn lock_file(lock_filename: &Path, lock_timeout: Duration) -> Result<File, String> {
    let start = Instant::now();
    loop {
        let file = File::create(lock_filename).map_err(|e| e.to_string())?;
        match file.try_lock_exclusive() {
            Ok(true) => return Ok(file),
            Ok(false) => {
                if start.elapsed() >= lock_timeout {
                    return Err(
                        format!("Timeout waiting for lockfile after {lock_timeout:?}").to_string(),
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // TODO: how do I test this?
            Err(error_message) => {
                return Err(error_message.to_string()); // TODO: how do I test this?
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
    let mut child = child_command.spawn().map_err(|e| e.to_string())?; // TODO: how do I test this?

    let exit_status = match timeout {
        Some(duration) => match child.wait_timeout(duration).map_err(|e| e.to_string())? {
            // TODO: how do I test this?
            Some(status) => Ok(status),
            None => {
                let pgid = Pid::from_raw(child.id() as i32);
                // TODO: send SIGINT first to give time for a graceful exit, then send SIGKILL
                // after 1 second.
                killpg(pgid, Signal::SIGKILL).map_err(|e| e.to_string())?; // TODO: how do I test this?
                child.wait().map_err(|e| e.to_string())?; // TODO: how do I test this?
                Err(format!("Command timed out after {duration:?}"))
            }
        },
        None => child.wait().map_err(|e| e.to_string()), // TODO: how do I test this?
    }?;

    exit_status
        .code()
        .ok_or_else(|| "Command terminated by signal".to_string())
}

fn realmain(args: Args) -> i32 {
    println!("tmux_window_name: {:?}", args.tmux_window_name);
    println!("lockfile: {:?}", args.lockfile);
    println!("lock_timeout: {:?}", args.lock_timeout_ms);
    println!("command_timeout: {:?}", args.command_timeout_ms);
    println!("directory: {:?}", args.directory);
    println!("command: {:?}", args.command);

    let _lock_file = if let Some(lockfile_path) = &args.lockfile {
        let lock_timeout = Duration::from_millis(args.lock_timeout_ms.unwrap_or(0));
        match lock_file(Path::new(lockfile_path), lock_timeout) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("Error: {}", e);
                return 1;
            }
        }
    } else {
        None
    };

    let mut command_to_run = args.command;
    if args.shell {
        command_to_run.insert(0, "sh".to_string());
        command_to_run.insert(1, "-c".to_string());
    }
    let command_timeout = args.command_timeout_ms.map(Duration::from_millis);
    match run_command(&command_to_run, command_timeout, args.directory.as_ref()) {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("Error: {}", e); // TODO: how do I test this?
            1
        }
    }
}

fn main() {
    std::process::exit(realmain(Args::parse()))
}

#[cfg(test)]
mod realmain {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_realmain() {
        let temp_file = NamedTempFile::new().unwrap();
        let result = realmain(Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=foo",
            &format!("--lockfile={}", temp_file.path().to_str().unwrap()),
            "--lock_timeout_ms=100",
            "--command_timeout_ms=100",
            "--directory=/tmp",
            "echo",
            "foo",
        ]));
        assert_eq!(result, 0);
    }

    #[test]
    fn test_realmain_lock_timeout() {
        let temp_file = NamedTempFile::new().unwrap();
        let lock_path = temp_file.path();
        let _lock = lock_file(lock_path, Duration::from_millis(100)).unwrap();
        let result = realmain(Args::parse_from(vec![
            "argv0",
            "--lockfile",
            lock_path.to_str().unwrap(),
            "--lock_timeout_ms=100",
            "echo",
            "foo",
        ]));
        assert_eq!(result, 1);
    }

    #[test]
    fn test_realmain_with_shell() {
        let result = realmain(Args::parse_from(vec![
            "argv0", "--shell", "echo", "foo", "bar",
        ]));
        assert_eq!(result, 0);
    }
}

#[cfg(test)]
mod run_command {
    use super::*;

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

    #[test]
    fn test_run_command_not_found() {
        let command = vec!["command_that_does_not_exist".to_string()];
        let result = run_command(&command, None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No such file or directory"));
    }

    #[test]
    fn test_run_command_terminated_by_signal() {
        let command = vec!["bash".to_string(), "-c".to_string(), "kill $$".to_string()];
        let result = run_command(&command, None, None);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Command terminated by signal");
    }
}

#[cfg(test)]
mod lock_file {
    use super::*;
    use std::env;
    use std::thread;

    #[test]
    fn test_lock_file_timeout() {
        let mut temp_file = env::temp_dir();
        temp_file.push("test_lock_file_timeout.lock");

        let _lock = lock_file(&temp_file, Duration::from_millis(200)).unwrap();

        let lock_result = thread::spawn(move || lock_file(&temp_file, Duration::from_micros(500)))
            .join()
            .unwrap();

        assert!(lock_result.is_err());
        assert!(
            lock_result
                .unwrap_err()
                .contains("Timeout waiting for lockfile after")
        );
    }

    #[test]
    fn test_lock_file_error() {
        let lock_result = lock_file(Path::new("/dev/fd"), Duration::from_secs(1));
        assert!(lock_result.is_err());
        assert!(lock_result.unwrap_err().contains("Is a directory"));
    }
}

#[cfg(test)]
mod clap_test {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify() {
        Args::command().debug_assert();
    }

    #[test]
    fn parse_args() {
        // Checks that I've configured the parser correctly.
        let args = Args::parse_from(vec!["argv0", "echo"]);
        assert_eq!(vec!["echo".to_string()], args.command);
        assert!(!args.shell);

        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name",
            "asdf",
            "--lockfile",
            "qwerty",
            "--lock_timeout_ms",
            "123",
            "--command_timeout_ms",
            "456",
            "--directory",
            "/no/where",
            "--shell",
            "echo",
            "foo",
            "bar",
        ]);
        assert_eq!(Some("asdf"), args.tmux_window_name.as_deref());
        assert_eq!(Some("qwerty"), args.lockfile.as_deref());
        assert_eq!(Some(123), args.lock_timeout_ms);
        assert_eq!(Some(456), args.command_timeout_ms);
        assert_eq!(Some("/no/where"), args.directory.as_deref());
        assert_eq!(
            vec!["echo".to_string(), "foo".to_string(), "bar".to_string()],
            args.command
        );
    }
}
