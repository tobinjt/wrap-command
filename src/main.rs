use clap::Parser;
use fs4::fs_std::FileExt;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::env;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// A program that wraps a command, and ensures that only one instance of the
/// command is running at a time.
#[derive(Parser, Debug, Clone)]
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
            Err(error_message) => {
                // I can't test this without making try_lock_exclusive() fail, which looks
                // ~impossible from reading the source.
                return Err(error_message.to_string());
            }
        }
    }
}

#[allow(dead_code)]
fn make_tmux_command(args: Args) -> Vec<String> {
    let mut full_command = vec![
        "tmux".to_string(),
        "new-window".to_string(),
        "-n".to_string(),
        args.tmux_window_name
            .expect("Internal error: make_tmux_command called without tmux_window_name"),
        env::current_exe()
            .expect("cannot determine current executable")
            .display()
            .to_string(),
    ];
    if let Some(directory) = args.directory {
        full_command.extend_from_slice(&["--directory".to_string(), directory.to_string()]);
    }
    if let Some(lockfile) = args.lockfile {
        full_command.extend_from_slice(&["--lockfile".to_string(), lockfile.to_string()]);
    }
    if let Some(lock_timeout_ms) = args.lock_timeout_ms {
        full_command
            .extend_from_slice(&["--lock_timeout_ms".to_string(), lock_timeout_ms.to_string()]);
    }
    if let Some(command_timeout_ms) = args.command_timeout_ms {
        full_command.extend_from_slice(&[
            "--command_timeout_ms".to_string(),
            command_timeout_ms.to_string(),
        ]);
    }
    if args.shell {
        full_command.extend_from_slice(&["--shell".to_string()]);
    }
    full_command.extend_from_slice(&args.command);
    full_command
}

fn run_command(args: &Args) -> Result<i32, String> {
    let _lock_file = if let Some(lockfile_path) = &args.lockfile {
        let lock_timeout = Duration::from_millis(args.lock_timeout_ms.unwrap_or(0));
        Some(lock_file(Path::new(lockfile_path), lock_timeout)?)
    } else {
        None
    };

    let mut child_command = Command::new(&args.command[0]);
    child_command.args(&args.command[1..]);
    if let Some(dir) = &args.directory {
        child_command.current_dir(dir);
    }
    child_command.process_group(0);

    let mut child = child_command.spawn().map_err(|e| e.to_string())?;

    let timeout = args.command_timeout_ms.map(Duration::from_millis);
    let exit_status = match timeout {
        Some(duration) => {
            match child.wait_timeout(duration).map_err(|e| e.to_string())? {
                Some(status) => Ok(status),
                None => {
                    let pgid = Pid::from_raw(child.id() as i32);
                    // TODO: send SIGINT first to give time for a graceful exit, then send SIGKILL
                    // after 1 second.
                    // I can't test this without causing killpg() to fail, which would require
                    // dependency injection I guess.  Maybe I could inject `Command::new` instead?
                    killpg(pgid, Signal::SIGKILL).map_err(|e| e.to_string())?;
                    // Likewise, I'd need to inject `Command::new`.
                    child.wait().map_err(|e| e.to_string())?;
                    Err(format!("Command timed out after {duration:?}"))
                }
            }
        }
        // Likewise, I'd need to inject `Command::new`.
        None => child.wait().map_err(|e| e.to_string()),
    }?;
    exit_status
        .code()
        .ok_or_else(|| "Command terminated by signal".to_string())
}

fn make_command_to_run(args: Args) -> Args {
    if args.tmux_window_name.is_some() {
        Args {
            command: make_tmux_command(args),
            tmux_window_name: None,
            lockfile: None,
            lock_timeout_ms: None,
            command_timeout_ms: None,
            directory: None,
            shell: false,
        }
    } else if args.shell {
        let mut command_with_shell = vec!["sh".to_string(), "-c".to_string()];
        command_with_shell.extend_from_slice(&args.command);
        Args {
            command: command_with_shell,
            ..args.clone()
        }
    } else {
        args.clone()
    }
}

fn realmain(args: Args) -> i32 {
    let args_for_command = make_command_to_run(args);

    match run_command(&args_for_command) {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn main() {
    std::process::exit(realmain(Args::parse()))
}

#[cfg(test)]
mod make_tmux_command {
    use super::*;

    #[test]
    fn test_make_tmux_command_basic() {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=my_window",
            "echo",
            "hello",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()
            .expect("cannot determine current executable")
            .display()
            .to_string();
        assert_eq!(
            result,
            vec![
                "tmux",
                "new-window",
                "-n",
                "my_window",
                &current_exe,
                "echo",
                "hello"
            ]
        );
    }

    #[test]
    fn test_make_tmux_command_all_args() {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=another_window",
            "--lockfile=/tmp/foo.lock",
            "--lock_timeout_ms=1000",
            "--command_timeout_ms=5000",
            "--directory=/tmp",
            "--shell",
            "ls",
            "-la",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()
            .expect("cannot determine current executable")
            .display()
            .to_string();
        assert_eq!(
            result,
            vec![
                "tmux",
                "new-window",
                "-n",
                "another_window",
                &current_exe,
                "--directory",
                "/tmp",
                "--lockfile",
                "/tmp/foo.lock",
                "--lock_timeout_ms",
                "1000",
                "--command_timeout_ms",
                "5000",
                "--shell",
                "ls",
                "-la"
            ]
        );
    }

    #[test]
    #[should_panic(expected = "Internal error: make_tmux_command called without tmux_window_name")]
    fn test_make_tmux_command_no_window_name() {
        let args = Args::parse_from(vec!["argv0", "echo", "hello"]);
        make_tmux_command(args);
    }
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

    #[test]
    fn test_realmain_command_terminated_by_signal() {
        let result = realmain(Args::parse_from(vec!["argv0", "--shell", "kill -9 $$"]));
        assert_eq!(result, 1);
    }
}

#[cfg(test)]
mod run_command {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_run_command_success() {
        let args = Args::parse_from(vec!["argv0", "echo", "foo"]);
        let result = run_command(&args);
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_run_command_lock_timeout() {
        let temp_file = NamedTempFile::new().unwrap();
        let lock_path = temp_file.path();
        let _lock = lock_file(lock_path, Duration::from_millis(100)).unwrap();

        let args = Args::parse_from(vec![
            "argv0",
            "--lockfile",
            lock_path.to_str().unwrap(),
            "--lock_timeout_ms=100",
            "echo",
            "foo",
        ]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Timeout waiting for lockfile"));
    }

    #[test]
    fn test_run_command_timeout() {
        let args = Args::parse_from(vec!["argv0", "--command_timeout_ms", "100", "sleep", "2"]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Command timed out after 100ms");
    }

    #[test]
    fn test_run_command_success_with_timeout() {
        let args = Args::parse_from(vec![
            "argv0",
            "--command_timeout_ms",
            "2000",
            "sleep",
            "0.1",
        ]);
        let result = run_command(&args);
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_run_command_fail() {
        let args = Args::parse_from(vec!["argv0", "false"]);
        let result = run_command(&args);
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_run_command_in_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("foo.txt");
        File::create(file_path).unwrap();

        let args = Args::parse_from(vec![
            "argv0",
            "--directory",
            temp_dir.path().to_str().unwrap(),
            "test",
            "-f",
            "foo.txt",
        ]);
        let result = run_command(&args);
        assert_eq!(result.unwrap(), 0);

        let args_fail = Args::parse_from(vec!["argv0", "test", "-f", "foo.txt"]);
        let result_fail = run_command(&args_fail);
        assert_eq!(result_fail.unwrap(), 1);
    }

    #[test]
    fn test_run_command_not_found() {
        let args = Args::parse_from(vec!["argv0", "command_that_does_not_exist"]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No such file or directory"));
    }

    #[test]
    fn test_run_command_terminated_by_signal() {
        let args = Args::parse_from(vec!["argv0", "bash", "-c", "kill $$"]);
        let result = run_command(&args);
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
mod make_command_to_run {
    use super::*;

    #[test]
    fn test_make_command_to_run_tmux() {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=my_window",
            "--lockfile=/tmp/foo.lock",
            "echo",
            "hello",
        ]);
        let result_args = make_command_to_run(args);
        let current_exe = env::current_exe()
            .expect("cannot determine current executable")
            .display()
            .to_string();
        assert_eq!(
            result_args.command,
            vec![
                "tmux",
                "new-window",
                "-n",
                "my_window",
                &current_exe,
                "--lockfile",
                "/tmp/foo.lock",
                "echo",
                "hello"
            ]
        );
        assert!(result_args.tmux_window_name.is_none());
        assert!(result_args.lockfile.is_none());
        assert!(result_args.lock_timeout_ms.is_none());
        assert!(result_args.command_timeout_ms.is_none());
        assert!(result_args.directory.is_none());
        assert!(!result_args.shell);
    }

    #[test]
    fn test_make_command_to_run_shell() {
        let args = Args::parse_from(vec!["argv0", "--shell", "echo", "foo", "bar"]);
        let result_args = make_command_to_run(args);
        assert_eq!(result_args.command, vec!["sh", "-c", "echo", "foo", "bar"]);
        assert!(result_args.shell);
    }

    #[test]
    fn test_make_command_to_run_no_modification() {
        let args = Args::parse_from(vec!["argv0", "--lockfile=/tmp/foo.lock", "echo", "hello"]);
        let original_args = args.clone();
        let result_args = make_command_to_run(args);
        assert_eq!(result_args.command, original_args.command);
        assert_eq!(result_args.lockfile, original_args.lockfile);
        assert_eq!(result_args.lock_timeout_ms, original_args.lock_timeout_ms);
        assert_eq!(
            result_args.command_timeout_ms,
            original_args.command_timeout_ms
        );
        assert_eq!(result_args.directory, original_args.directory);
        assert_eq!(result_args.shell, original_args.shell);
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
