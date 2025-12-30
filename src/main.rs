use clap::Parser;
use fs4::fs_std::FileExt;
use nix::sys::signal::killpg;
use nix::unistd::Pid;
use std::env;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

const LONG_ABOUT: &str = "A program that wraps a command, optionally:
- using a lock to ensure only one instance is running (--lockfile)
  - either failing immediately if the lock is held or waiting for a
    configurable time for the lock to be released (--lock_timeout_ms)
- running the command with a timeout (--command_timeout_ms)
  - the signal to send can be specified with --signal, it defaults
    to SIGTERM (15).
  - the time to wait for the child to exit after sending the signal
    can be specified with --signal_timeout_ms, it defaults to 1000ms.
    If the child process is still running after this time, it is
    killed with SIGKILL (9).
- running the command from a different directory (--directory)
- passing the command to `sh -c` so shell metacharacters like && or
  $() can be used (--shell)
- running the command in a new tmux window (--tmux_window_name)
Any combination of unindented flags is supported.  The indented flags
require the flag they are indented under.";

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about=LONG_ABOUT)]
struct Args {
    /// The name of the tmux window to use.
    #[arg(long = "tmux_window_name")]
    tmux_window_name: Option<String>,

    /// The path to the lockfile.
    #[arg(long = "lockfile")]
    lockfile: Option<String>,

    /// The lock_timeout in milliseconds.
    #[arg(long = "lock_timeout_ms", requires = "lockfile")]
    lock_timeout_ms: Option<u64>,

    /// The command_timeout in milliseconds.
    #[arg(long = "command_timeout_ms")]
    command_timeout_ms: Option<u64>,

    /// The signal to send to the command if it times out. Can be a signal name (e.g. "SIGTERM") or a signal number (e.g. "15").
    /// Defaults to SIGINT (2) if not specified.
    #[arg(
        long = "signal",
        requires = "command_timeout_ms",
        default_value = "SIGTERM"
    )]
    signal: Option<String>,

    /// The time in milliseconds to wait for the child to exit after sending signal.
    #[arg(
        long = "signal_timeout_ms",
        requires = "command_timeout_ms",
        default_value_t = 1000
    )]
    signal_timeout_ms: u64,

    /// The directory to run the command in.
    #[arg(long = "directory")]
    directory: Option<String>,

    /// Prepend `["sh", "-c"]` to the command.  Doesn't otherwise modify the command.
    #[arg(long = "shell")]
    shell: bool,

    /// Ping this URL on success, e.g. https://hc-ping.com/....
    #[arg(long = "success_url")]
    success_url: Option<String>,

    /// Ping this URL on failure, e.g. https://hc-ping.com/....
    #[arg(long = "failure_url")]
    failure_url: Option<String>,

    /// The command to run.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

fn ping_url(url: &str) {
    let client = reqwest::blocking::Client::new();
    if let Err(e) = client.get(url).send() {
        eprintln!("Failed to ping URL {}: {}", url, e);
    }
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

fn make_tmux_command(args: Args) -> Vec<String> {
    let mut full_command = vec![
        "tmux".to_string(),
        "new-window".to_string(),
        "-d".to_string(),
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
        full_command.extend_from_slice(&[
            "--signal".to_string(),
            args.signal
                .expect("Internal error: signal argument should always be set"),
        ]);
        full_command.extend_from_slice(&[
            "--signal_timeout_ms".to_string(),
            args.signal_timeout_ms.to_string(),
        ]);
    }
    if args.shell {
        full_command.extend_from_slice(&["--shell".to_string()]);
    }
    if let Some(success_url) = args.success_url {
        full_command.extend_from_slice(&["--success_url".to_string(), success_url]);
    }
    if let Some(failure_url) = args.failure_url {
        full_command.extend_from_slice(&["--failure_url".to_string(), failure_url]);
    }
    full_command.extend_from_slice(&args.command);
    full_command
}

fn kill_child_process_group(
    child: &mut std::process::Child,
    signal_name: Option<&str>,
    signal_timeout_ms: u64,
) -> Result<(), String> {
    let pgid = Pid::from_raw(child.id() as i32);
    let signal: nix::sys::signal::Signal = signal_name
        .expect("internal error: missing signal name")
        .parse()
        .map_err(|e| format!("Invalid signal: {e}"))?;
    println!("signal: {}", signal);
    // I can't test this without causing killpg() to fail, which would require
    // dependency injection I guess.  Maybe I could inject `Command::new` instead?
    killpg(pgid, signal).map_err(|e| e.to_string())?;

    let timeout = Duration::from_millis(signal_timeout_ms);
    // I can't test this without causing wait_timeout() to fail, which would require
    // dependency injection I guess.  Maybe I could inject `Command::new` instead?
    match child.wait_timeout(timeout).map_err(|e| e.to_string())? {
        Some(_) => Ok(()),
        None => {
            // I can't test this without causing killpg() to fail, which would require
            // dependency injection I guess.  Maybe I could inject `Command::new` instead?
            killpg(pgid, nix::sys::signal::Signal::SIGKILL).map_err(|e| e.to_string())?;
            // I can't test this without causing wait() to fail, which would require
            // dependency injection I guess.  Maybe I could inject `Command::new` instead?
            child.wait().map_err(|e| e.to_string())?;
            Ok(())
        }
    }
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
        // I can't test this without causing wait_timeout() to fail, which would require
        // dependency injection I guess.  Maybe I could inject `Command::new` instead?
        Some(duration) => match child.wait_timeout(duration).map_err(|e| e.to_string())? {
            Some(status) => Ok(status),
            None => {
                kill_child_process_group(
                    &mut child,
                    args.signal.as_deref(),
                    args.signal_timeout_ms,
                )?;
                Err(format!("Command timed out after {duration:?}"))
            }
        },
        // I can't test this without causing wait() to fail, which would require
        // dependency injection I guess.  Maybe I could inject `Command::new` instead?
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
            signal: None,
            signal_timeout_ms: 1000,
            directory: None,
            shell: false,
            success_url: None,
            failure_url: None,
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
        Ok(exit_code) => {
            if exit_code == 0 {
                if let Some(url) = &args_for_command.success_url {
                    ping_url(url);
                }
            } else if let Some(url) = &args_for_command.failure_url {
                ping_url(url);
            }
            exit_code
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            if let Some(url) = &args_for_command.failure_url {
                ping_url(url);
            }
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
                "-d",
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
            "--signal=SIGTERM",
            "--signal_timeout_ms=2000",
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
                "-d",
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
                "--signal",
                "SIGTERM",
                "--signal_timeout_ms",
                "2000",
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

    #[test]
    fn test_make_tmux_command_forward_urls() {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=window",
            "--success_url=http://success",
            "--failure_url=http://failure",
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
                "-d",
                "-n",
                "window",
                &current_exe,
                "--success_url",
                "http://success",
                "--failure_url",
                "http://failure",
                "echo",
                "hello"
            ]
        );
    }
}

#[cfg(test)]
mod ping_tests {
    use super::*;
    use mockito::Server;

    #[test]
    fn test_ping_success() {
        let mut server = Server::new();
        let expected_request = server.mock("GET", "/success").with_status(200).create();

        let url = format!("{}/success", server.url());
        let args = Args::parse_from(vec!["argv0", "--success_url", &url, "true"]);

        let result = realmain(args);
        assert_eq!(result, 0);
        // Check that the expected request was made.
        expected_request.assert();
    }

    #[test]
    fn test_ping_failure() {
        let mut server = Server::new();
        let expected_request = server.mock("GET", "/failure").with_status(200).create();

        let url = format!("{}/failure", server.url());
        let args = Args::parse_from(vec!["argv0", "--failure_url", &url, "false"]);

        let result = realmain(args);
        assert_eq!(result, 1);
        expected_request.assert();
    }

    #[test]
    fn test_ping_failure_on_command_error() {
        // Run a command that doesn't exist to trigger Err() in run_command
        let mut server = Server::new();
        let expected_request = server.mock("GET", "/failure").with_status(200).create();

        let url = format!("{}/failure", server.url());
        let args = Args::parse_from(vec![
            "argv0",
            "--failure_url",
            &url,
            "command_does_not_exist",
        ]);

        let result = realmain(args);
        assert_eq!(result, 1);
        expected_request.assert();
    }

    #[test]
    fn test_success_does_not_trigger_failure_url() {
        let mut server = Server::new();
        let m_failure = server.mock("GET", "/failure").expect(0).create();
        let m_success = server.mock("GET", "/success").with_status(200).create();

        let success_url = format!("{}/success", server.url());
        let failure_url = format!("{}/failure", server.url());

        let args = Args::parse_from(vec![
            "argv0",
            "--success_url",
            &success_url,
            "--failure_url",
            &failure_url,
            "true",
        ]);
        realmain(args);

        m_failure.assert();
        m_success.assert();
    }

    #[test]
    fn test_failure_does_not_trigger_success_url() {
        let mut server = Server::new();
        let m_failure = server.mock("GET", "/failure").with_status(200).create();
        let m_success = server.mock("GET", "/success").expect(0).create();

        let success_url = format!("{}/success", server.url());
        let failure_url = format!("{}/failure", server.url());

        let args = Args::parse_from(vec![
            "argv0",
            "--success_url",
            &success_url,
            "--failure_url",
            &failure_url,
            "false",
        ]);
        realmain(args);

        m_failure.assert();
        m_success.assert();
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

    #[test]
    fn test_run_command_invalid_signal() {
        let args = Args::parse_from(vec![
            "argv0",
            "--command_timeout_ms",
            "10",
            "--signal",
            "INVALID_SIGNAL",
            "sleep",
            "1",
        ]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid signal"));
    }

    #[test]
    fn test_run_command_signal_timeout_kill() {
        // This command ignores SIGTERM and sleeps for 2 seconds.
        // We set command_timeout to 100ms, so it will timeout.
        // We set signal to SIGTERM.
        // We set signal_timeout to 200ms.
        // It should receive SIGTERM, ignore it, wait 200ms, get SIGKILL, and die.
        let args = Args::parse_from(vec![
            "argv0",
            "--command_timeout_ms",
            "100",
            "--signal",
            "SIGTERM",
            "--signal_timeout_ms",
            "200",
            "sh",
            "-c",
            "trap '' TERM; sleep 2",
        ]);
        let start = Instant::now();
        let result = run_command(&args);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Command timed out after 100ms");
        // Elapsed should be roughly 100ms (command timeout) + 200ms (signal timeout)
        // It should be at least 300ms.
        // allow some buffer for CI flakiness, but definitely > 100ms.
        assert!(elapsed >= Duration::from_millis(250));
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
                "-d",
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
        assert_eq!(1000, result_args.signal_timeout_ms);
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
        assert_eq!(
            result_args.signal_timeout_ms,
            original_args.signal_timeout_ms
        );
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
            "--signal",
            "SIGTERM",
            "--signal_timeout_ms",
            "789",
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
        assert_eq!(Some("SIGTERM"), args.signal.as_deref());
        assert_eq!(789, args.signal_timeout_ms);
        assert_eq!(Some("/no/where"), args.directory.as_deref());
        assert_eq!(
            vec!["echo".to_string(), "foo".to_string(), "bar".to_string()],
            args.command
        );
    }

    #[test]
    fn test_parse_args_default_signal() {
        let args = Args::parse_from(vec!["argv0", "--command_timeout_ms", "100", "echo", "foo"]);
        assert_eq!(Some("SIGTERM"), args.signal.as_deref());
        assert_eq!(1000, args.signal_timeout_ms);
    }

    #[test]
    fn test_lock_timeout_requires_lockfile() {
        let result = Args::try_parse_from(vec!["argv0", "--lock_timeout_ms=100", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--lockfile"));
    }

    #[test]
    fn test_signal_requires_command_timeout_ms() {
        let result = Args::try_parse_from(vec!["argv0", "--signal=SIGTERM", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--command_timeout_ms"));
    }

    #[test]
    fn test_signal_timeout_requires_command_timeout_ms() {
        let result = Args::try_parse_from(vec!["argv0", "--signal_timeout_ms=100", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--command_timeout_ms"));
    }
}
