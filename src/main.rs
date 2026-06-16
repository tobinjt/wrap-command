use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use fs4::fs_std::FileExt;
use nix::sys::signal::killpg;
use nix::unistd::Pid;
use std::env;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

#[cfg(target_os = "macos")]
const CAFFEINATE_CMD: &[&str] = &["caffeinate", "-i"];

#[cfg(target_os = "linux")]
const CAFFEINATE_CMD: &[&str] = &["systemd-inhibit", "--what=idle"];

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("wrap-command only supports MacOS and Linux");

fn parse_duration(arg: &str) -> Result<Duration, String> {
    fundu::parse_duration(arg).map_err(|e| e.to_string())
}

const LONG_ABOUT: &str = "A program that wraps a command, optionally:
- using a lock to ensure only one instance is running (--lockfile)
  - either failing immediately if the lock is held or waiting for a
    configurable time for the lock to be released (--lock_timeout)
- running the command with a timeout (--command_timeout)
  - the signal to send can be specified with --signal, it defaults
    to SIGTERM (15).
  - the time to wait for the child to exit after sending the signal
    can be specified with --signal_timeout, it defaults to 1s.
    If the child process is still running after this time, it is
    killed with SIGKILL (9).
- running the command from a different directory (--directory)
- passing the command to `sh -c` so shell metacharacters like && or
  $() can be used (--shell)
- running the command in a new tmux window (--tmux_window_name)
- preventing the system from sleeping (--caffeinate)
- waiting for network connectivity (--network_check_timeout)
  - waits for http://clients3.google.com/generate_204 to be reachable.
  - 0s means wait forever, otherwise timeout after that duration.
- waiting for the user to press enter after the command has finished (--wait)
Any combination of unindented flags is supported.  The indented flags
require the flag they are indented under.

Timeout / Duration Formats:
  Timeouts can be specified as a number followed by an optional unit:
  - ns, us (or µs), ms, s, m, h, d, w, M, y (e.g., 500ms, 1.5s, 2h).
  - If no unit is specified, seconds (s) is assumed (e.g., 10 is parsed as 10s).
  - Standard floating-point values are supported (e.g., 1.5s, .5h, 2e-3s).
  - Multiple units or negative values are not supported (e.g., 1s2ms, -5s are invalid).";

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about=LONG_ABOUT)]
struct Args {
    /// The directory to run the command in.
    #[arg(long = "directory", help_heading = "Execution & Environment")]
    directory: Option<String>,

    /// Prepend `["sh", "-c"]` to the command.  Doesn't otherwise modify the
    /// command, in particular if you use `--shell` with multiple arguments they
    /// will be passed as multiple arguments to `sh`. E.g.
    ///   "wrap-command" "--shell" "ls" "foo" "bar"
    /// will result in:
    ///   "sh" "-c" "ls" "foo" "bar"
    /// *not*:
    ///   "sh" "-c" "ls foo bar"
    #[arg(
        long = "shell",
        verbatim_doc_comment,
        help_heading = "Execution & Environment"
    )]
    shell: bool,

    /// Run the command in a new tmux window with the specified name.
    #[arg(long = "tmux_window_name", help_heading = "Execution & Environment")]
    tmux_window_name: Option<String>,

    /// Prevent the system from sleeping while the command is running.
    #[arg(long = "caffeinate", help_heading = "Execution & Environment")]
    caffeinate: bool,

    /// Wait for network connectivity before running the command.
    /// 0s: wait forever. Otherwise: timeout duration.
    #[arg(long = "network_check_timeout", value_parser = parse_duration, help_heading = "Network & Connectivity")]
    network_check_timeout: Option<Duration>,

    /// URL to check for network connectivity.
    #[arg(
        long = "network_check_url",
        hide = true,
        default_value = "http://clients3.google.com/generate_204",
        help_heading = "Network & Connectivity"
    )]
    network_check_url: String,

    /// The path to the lockfile.
    #[arg(long = "lockfile", help_heading = "Exclusivity & Locking")]
    lockfile: Option<String>,

    /// The lock_timeout duration.
    #[arg(long = "lock_timeout", requires = "lockfile", value_parser = parse_duration, help_heading = "Exclusivity & Locking")]
    lock_timeout: Option<Duration>,

    /// The command_timeout duration.
    #[arg(long = "command_timeout", value_parser = parse_duration, help_heading = "Timeouts & Signal Control")]
    command_timeout: Option<Duration>,

    /// The signal to send to the command if it times out. Can be a signal
    /// name (e.g. "SIGTERM") or a signal number (e.g. "15").
    /// Defaults to SIGINT (2) if not specified.
    #[arg(
        long = "signal",
        requires = "command_timeout",
        default_value = "SIGTERM",
        help_heading = "Timeouts & Signal Control"
    )]
    signal: Option<String>,

    /// The time to wait for the child to exit after sending signal.
    #[arg(
        long = "signal_timeout",
        requires = "command_timeout",
        default_value = "1s",
        value_parser = parse_duration,
        help_heading = "Timeouts & Signal Control"
    )]
    signal_timeout: Duration,

    /// Ping this URL on success, e.g. https://hc-ping.com/....
    #[arg(long = "success_url", help_heading = "Notifications & Hooks")]
    success_url: Option<String>,

    /// Ping this URL on failure, e.g. https://hc-ping.com/....
    #[arg(long = "failure_url", help_heading = "Notifications & Hooks")]
    failure_url: Option<String>,

    /// Number of retries when pinging success/failure URLs.
    #[arg(
        long = "url_retry_count",
        default_value_t = 5,
        help_heading = "Notifications & Hooks"
    )]
    url_retry_count: u32,

    /// Delay between retries when pinging success/failure URLs.
    #[arg(long = "url_retry_delay", default_value = "1s", value_parser = parse_duration, help_heading = "Notifications & Hooks")]
    url_retry_delay: Duration,

    /// Wait for the user to press enter after the command has finished.
    #[arg(long = "wait", help_heading = "User Interaction")]
    wait: bool,

    /// Output shell completion code for the specified shell.
    #[arg(long = "output_shell_completion", help_heading = "Shell Completion")]
    output_shell_completion: Option<Shell>,

    /// The command to run.
    #[arg(
        trailing_var_arg = true,
        required_unless_present = "output_shell_completion"
    )]
    command: Vec<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            directory: None,
            shell: false,
            tmux_window_name: None,
            caffeinate: false,
            network_check_timeout: None,
            network_check_url: "http://clients3.google.com/generate_204".to_string(),
            lockfile: None,
            lock_timeout: None,
            command_timeout: None,
            signal: None,
            signal_timeout: Duration::from_secs(1),
            success_url: None,
            failure_url: None,
            url_retry_count: 5,
            url_retry_delay: Duration::from_secs(1),
            wait: false,
            output_shell_completion: None,
            command: Vec::new(),
        }
    }
}

fn ping_url(url: &str, retry_count: u32, retry_delay: Duration) {
    let mut attempts = 0;
    loop {
        match ureq::get(url).call() {
            Ok(_) => break,
            Err(e) => {
                if attempts >= retry_count {
                    eprintln!(
                        "Failed to ping URL {} after {} retries: {}",
                        url, retry_count, e
                    );
                    break;
                }
                std::thread::sleep(retry_delay);
                attempts += 1;
            }
        }
    }
}

fn lock_file(lock_filename: &Path, lock_timeout: Duration) -> Result<File, String> {
    let start = Instant::now();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o600)
        .truncate(false)
        .open(lock_filename)
        .map_err(|e| e.to_string())?;
    loop {
        match file.try_lock_exclusive() {
            Ok(true) => return Ok(file),
            Ok(false) => {
                if start.elapsed() >= lock_timeout {
                    return Err(format!(
                        "Timeout waiting for lockfile after {lock_timeout:?}"
                    ));
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

fn push_opt<T: std::fmt::Display>(vec: &mut Vec<String>, flag: &str, opt: Option<T>) {
    if let Some(val) = opt {
        vec.push(flag.to_string());
        vec.push(val.to_string());
    }
}

fn make_tmux_command(args: Args) -> Vec<String> {
    let mut full_command = Vec::with_capacity(args.command.len() + 33);
    full_command.push("tmux".to_string());
    full_command.push("new-window".to_string());
    full_command.push("-d".to_string());
    full_command.push("-n".to_string());
    full_command.push(
        args.tmux_window_name
            .expect("Internal error: make_tmux_command called without tmux_window_name"),
    );
    full_command.push(
        env::current_exe()
            .expect("cannot determine current executable")
            .display()
            .to_string(),
    );

    push_opt(&mut full_command, "--directory", args.directory);
    push_opt(&mut full_command, "--lockfile", args.lockfile);
    push_opt(
        &mut full_command,
        "--lock_timeout",
        args.lock_timeout.map(|d| format!("{}ms", d.as_millis())),
    );
    if args.command_timeout.is_some() {
        push_opt(
            &mut full_command,
            "--command_timeout",
            args.command_timeout.map(|d| format!("{}ms", d.as_millis())),
        );
        full_command.push("--signal".to_string());
        full_command.push(
            args.signal
                .expect("Internal error: signal argument should always be set"),
        );
        full_command.push("--signal_timeout".to_string());
        full_command.push(format!("{}ms", args.signal_timeout.as_millis()));
    }
    if args.shell {
        full_command.push("--shell".to_string());
    }
    push_opt(&mut full_command, "--success_url", args.success_url);
    push_opt(&mut full_command, "--failure_url", args.failure_url);
    if args.url_retry_delay != Duration::from_secs(1) {
        full_command.push("--url_retry_delay".to_string());
        full_command.push(format!("{}ms", args.url_retry_delay.as_millis()));
    }
    if args.url_retry_count != 5 {
        full_command.push("--url_retry_count".to_string());
        full_command.push(args.url_retry_count.to_string());
    }
    if args.caffeinate {
        full_command.push("--caffeinate".to_string());
    }
    if args.wait {
        full_command.push("--wait".to_string());
    }
    push_opt(
        &mut full_command,
        "--network_check_timeout",
        args.network_check_timeout
            .map(|d| format!("{}ms", d.as_millis())),
    );
    if args.network_check_url != "http://clients3.google.com/generate_204" {
        full_command.push("--network_check_url".to_string());
        full_command.push(args.network_check_url);
    }
    full_command.extend(args.command);
    full_command
}

fn check_network_connectivity(url: &str, timeout: Duration) -> Result<(), String> {
    let mut builder = ureq::config::Config::builder();
    if !timeout.is_zero() {
        builder = builder.timeout_global(Some(timeout));
    }
    let agent = builder.build().new_agent();
    match agent.head(url).call() {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Network check failed for {}: {}", url, e)),
    }
}

fn kill_child_process_group(
    child: &mut std::process::Child,
    signal_name: Option<&str>,
    signal_timeout: Duration,
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

    // I can't test this without causing wait_timeout() to fail, which would require
    // dependency injection I guess.  Maybe I could inject `Command::new` instead?
    match child
        .wait_timeout(signal_timeout)
        .map_err(|e| e.to_string())?
    {
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

fn acquire_lock(args: &Args) -> Result<Option<File>, String> {
    if let Some(lockfile_path) = &args.lockfile {
        let lock_timeout = args.lock_timeout.unwrap_or(Duration::ZERO);
        Ok(Some(lock_file(Path::new(lockfile_path), lock_timeout)?))
    } else {
        Ok(None)
    }
}

fn run_command(args: &Args) -> Result<i32, String> {
    let _lock_file = acquire_lock(args)?;

    if let Some(timeout) = args.network_check_timeout {
        check_network_connectivity(&args.network_check_url, timeout)?;
    }

    manage_child_process(args)
}

fn manage_child_process(args: &Args) -> Result<i32, String> {
    let mut child_command = Command::new(&args.command[0]);
    child_command.args(&args.command[1..]);
    if let Some(dir) = &args.directory {
        child_command.current_dir(dir);
    }
    child_command.process_group(0);

    let mut child = child_command.spawn().map_err(|e| e.to_string())?;

    let timeout = args.command_timeout;
    let exit_status = match timeout {
        // I can't test this without causing wait_timeout() to fail, which would require
        // dependency injection I guess.  Maybe I could inject `Command::new` instead?
        Some(duration) => match child.wait_timeout(duration).map_err(|e| e.to_string())? {
            Some(status) => Ok(status),
            None => {
                kill_child_process_group(&mut child, args.signal.as_deref(), args.signal_timeout)?;
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
            ..Default::default()
        }
    } else {
        let mut command = args.command;
        if args.shell {
            let mut shell_command = Vec::with_capacity(command.len() + 2);
            shell_command.push("sh".to_string());
            shell_command.push("-c".to_string());
            shell_command.extend(command);
            command = shell_command;
        }
        if args.caffeinate {
            let mut caffeinate_command = Vec::with_capacity(command.len() + CAFFEINATE_CMD.len());
            caffeinate_command.extend(CAFFEINATE_CMD.iter().map(|s| s.to_string()));
            caffeinate_command.extend(command);
            command = caffeinate_command;
        }
        Args { command, ..args }
    }
}

fn realmain(args: Args) -> i32 {
    realmain_impl(args, &mut io::stdin().lock(), &mut io::stdout())
}

fn realmain_impl<R: io::BufRead, W: io::Write>(args: Args, reader: &mut R, writer: &mut W) -> i32 {
    if let Some(shell) = args.output_shell_completion {
        generate(shell, &mut Args::command(), "wrap-command", writer);
        return 0;
    }
    let args_for_command = make_command_to_run(args);

    let exit_code = match run_command(&args_for_command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    };

    let url = if exit_code == 0 {
        &args_for_command.success_url
    } else {
        &args_for_command.failure_url
    };

    if let Some(url) = url {
        ping_url(
            url,
            args_for_command.url_retry_count,
            args_for_command.url_retry_delay,
        );
    }

    if args_for_command.wait {
        let _ = writeln!(writer, "Press Enter to continue...");
        let mut _input = String::new();
        let _ = reader.read_line(&mut _input);
    }

    exit_code
}
fn main() {
    std::process::exit(realmain(Args::parse()))
}

#[cfg(test)]
mod make_tmux_command {
    use super::*;

    #[test]
    fn test_make_tmux_command_basic() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=my_window",
            "echo",
            "hello",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()?.display().to_string();
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
        Ok(())
    }

    #[test]
    fn test_make_tmux_command_all_args() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=another_window",
            "--lockfile=/tmp/foo.lock",
            "--lock_timeout=1000ms",
            "--command_timeout=5000ms",
            "--directory=/tmp",
            "--signal=SIGTERM",
            "--signal_timeout=2000ms",
            "--shell",
            "--caffeinate",
            "--url_retry_delay=2000ms",
            "--url_retry_count=10",
            "--network_check_timeout=3s",
            "--network_check_url=http://example.com",
            "ls",
            "-la",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()?.display().to_string();
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
                "--lock_timeout",
                "1000ms",
                "--command_timeout",
                "5000ms",
                "--signal",
                "SIGTERM",
                "--signal_timeout",
                "2000ms",
                "--shell",
                "--url_retry_delay",
                "2000ms",
                "--url_retry_count",
                "10",
                "--caffeinate",
                "--network_check_timeout",
                "3000ms",
                "--network_check_url",
                "http://example.com",
                "ls",
                "-la"
            ]
        );
        Ok(())
    }

    #[test]
    fn test_make_tmux_command_wait() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=window",
            "--wait",
            "echo",
            "hello",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()?.display().to_string();
        assert_eq!(
            result,
            vec![
                "tmux",
                "new-window",
                "-d",
                "-n",
                "window",
                &current_exe,
                "--wait",
                "echo",
                "hello"
            ]
        );
        Ok(())
    }

    #[test]
    #[should_panic(expected = "Internal error: make_tmux_command called without tmux_window_name")]
    fn test_make_tmux_command_no_window_name() {
        let args = Args::parse_from(vec!["argv0", "echo", "hello"]);
        make_tmux_command(args);
    }

    #[test]
    fn test_make_tmux_command_forward_urls() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=window",
            "--success_url=http://success",
            "--failure_url=http://failure",
            "echo",
            "hello",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()?.display().to_string();
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
        Ok(())
    }

    #[test]
    fn test_make_tmux_command_custom_network_check_url() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=window",
            "--network_check_url=http://custom.url",
            "echo",
            "hello",
        ]);
        let result = make_tmux_command(args);
        let current_exe = env::current_exe()?.display().to_string();
        assert_eq!(
            result,
            vec![
                "tmux",
                "new-window",
                "-d",
                "-n",
                "window",
                &current_exe,
                "--network_check_url",
                "http://custom.url",
                "echo",
                "hello"
            ]
        );
        Ok(())
    }
}

#[cfg(test)]
mod ping_tests {
    use super::*;
    use mockito::Server;

    #[test]
    fn test_ping_success() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let expected_request = server.mock("GET", "/success").with_status(200).create();

        let url = format!("{}/success", server.url());
        let args = Args::parse_from(vec!["argv0", "--success_url", &url, "true"]);

        let result = realmain(args);
        assert_eq!(result, 0);
        // Check that the expected request was made.
        expected_request.assert();
        Ok(())
    }

    #[test]
    fn test_ping_failure() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let expected_request = server.mock("GET", "/failure").with_status(200).create();

        let url = format!("{}/failure", server.url());
        let args = Args::parse_from(vec!["argv0", "--failure_url", &url, "false"]);

        let result = realmain(args);
        assert_eq!(result, 1);
        expected_request.assert();
        Ok(())
    }

    #[test]
    fn test_ping_failure_on_command_error() -> Result<(), Box<dyn std::error::Error>> {
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
        Ok(())
    }

    #[test]
    fn test_success_does_not_trigger_failure_url() -> Result<(), Box<dyn std::error::Error>> {
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
        Ok(())
    }

    #[test]
    fn test_failure_does_not_trigger_success_url() -> Result<(), Box<dyn std::error::Error>> {
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
        Ok(())
    }

    #[test]
    fn test_ping_url_error() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let expected_request = server
            .mock("GET", "/failure")
            .with_status(500)
            .expect(6)
            .create();

        let url = format!("{}/failure", server.url());
        let args = Args::parse_from(vec![
            "argv0",
            "--failure_url",
            &url,
            "--url_retry_delay",
            "0s",
            "false",
        ]);

        let result = realmain(args);
        assert_eq!(result, 1);
        expected_request.assert();
        Ok(())
    }

    #[test]
    fn test_ping_url_success_first_try() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let expected_request = server
            .mock("GET", "/success_first")
            .with_status(200)
            .expect(1)
            .create();
        let url = format!("{}/success_first", server.url());

        ping_url(&url, 2, Duration::ZERO);

        expected_request.assert();
        Ok(())
    }

    #[test]
    fn test_ping_url_exhausts_retries() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        // retry_count = 2 means it will try 1 + 2 = 3 times
        let expected_request = server
            .mock("GET", "/exhausts")
            .with_status(500)
            .expect(3)
            .create();
        let url = format!("{}/exhausts", server.url());

        ping_url(&url, 2, Duration::ZERO);

        expected_request.assert();
        Ok(())
    }

    #[test]
    fn test_ping_url_success_after_retry() -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{}", port);

        std::thread::spawn(move || {
            // 1st request: accept connection and return 500
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
            // 2nd request: accept connection and return 200
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });

        ping_url(&url, 2, Duration::from_millis(1));
        Ok(())
    }
}

#[cfg(test)]
mod realmain {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_realmain_with_tmux_window_name() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let tmux_mock_path = temp_dir.path().join("tmux");
        std::fs::write(&tmux_mock_path, "#!/bin/sh\nexit 0")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmux_mock_path, std::fs::Permissions::from_mode(0o755))?;
        }

        let old_path = env::var("PATH").ok();
        let new_path = match &old_path {
            Some(path) => format!("{}:{}", temp_dir.path().to_str().unwrap(), path),
            None => temp_dir.path().to_str().unwrap().to_string(),
        };
        unsafe {
            env::set_var("PATH", &new_path);
            env::set_var("TMUX", "1");
        }

        let temp_file = NamedTempFile::new()?;
        let result = realmain(Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=foo",
            &format!(
                "--lockfile={}",
                temp_file.path().to_str().ok_or("invalid path")?
            ),
            "--lock_timeout=100ms",
            "--command_timeout=100ms",
            "--directory=/tmp",
            "echo",
            "foo",
        ]));

        unsafe {
            if let Some(path) = old_path {
                env::set_var("PATH", path);
            } else {
                env::remove_var("PATH");
            }
            env::remove_var("TMUX");
        }

        assert_eq!(result, 0);
        Ok(())
    }

    #[test]
    fn test_realmain_lock_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let lock_path = temp_file.path();
        let _lock = lock_file(lock_path, Duration::from_millis(100))?;
        let result = realmain(Args::parse_from(vec![
            "argv0",
            "--lockfile",
            lock_path.to_str().ok_or("invalid path")?,
            "--lock_timeout=100ms",
            "echo",
            "foo",
        ]));
        assert_eq!(result, 1);
        Ok(())
    }

    #[test]
    fn test_realmain_with_shell() -> Result<(), Box<dyn std::error::Error>> {
        let result = realmain(Args::parse_from(vec![
            "argv0", "--shell", "echo", "foo", "bar",
        ]));
        assert_eq!(result, 0);
        Ok(())
    }

    #[test]
    fn test_realmain_command_terminated_by_signal() -> Result<(), Box<dyn std::error::Error>> {
        let result = realmain(Args::parse_from(vec!["argv0", "--shell", "kill -9 $$"]));
        assert_eq!(result, 1);
        Ok(())
    }

    #[test]
    fn test_realmain_fail_no_url() -> Result<(), Box<dyn std::error::Error>> {
        let result = realmain(Args::parse_from(vec!["argv0", "false"]));
        assert_eq!(result, 1);
        Ok(())
    }

    #[test]
    fn test_realmain_error_no_url() -> Result<(), Box<dyn std::error::Error>> {
        let result = realmain(Args::parse_from(vec!["argv0", "command_does_not_exist"]));
        assert_eq!(result, 1);
        Ok(())
    }

    #[test]
    fn test_realmain_success_no_url() -> Result<(), Box<dyn std::error::Error>> {
        let result = realmain(Args::parse_from(vec!["argv0", "true"]));
        assert_eq!(result, 0);
        Ok(())
    }

    #[test]
    fn test_realmain_output_shell_completion() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = Vec::new();
        let mut reader = std::io::Cursor::new(Vec::new());
        let result = realmain_impl(
            Args::parse_from(vec!["argv0", "--output_shell_completion", "bash"]),
            &mut reader,
            &mut buffer,
        );
        assert_eq!(result, 0);
        let output = String::from_utf8(buffer)?;
        assert!(output.contains("_wrap-command"));
        Ok(())
    }

    #[test]
    fn test_realmain_wait() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = Vec::new();
        let mut reader = std::io::Cursor::new(b"\n");
        let result = realmain_impl(
            Args::parse_from(vec!["argv0", "--wait", "echo", "foo"]),
            &mut reader,
            &mut buffer,
        );
        assert_eq!(result, 0);
        let output = String::from_utf8(buffer)?;
        assert!(output.contains("Press Enter to continue..."));
        assert_eq!(reader.position(), 1);
        Ok(())
    }
}

#[cfg(test)]
mod run_command {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_run_command_success() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "echo", "foo"]);
        let result = run_command(&args);
        assert_eq!(result?, 0);
        Ok(())
    }

    #[test]
    fn test_run_command_lock_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let lock_path = temp_file.path();
        let _lock = lock_file(lock_path, Duration::from_millis(100))?;

        let args = Args::parse_from(vec![
            "argv0",
            "--lockfile",
            lock_path.to_str().ok_or("invalid path")?,
            "--lock_timeout=100ms",
            "echo",
            "foo",
        ]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("Timeout waiting for lockfile")
        );
        Ok(())
    }

    #[test]
    fn test_run_command_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--command_timeout", "100ms", "sleep", "2"]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert_eq!(
            result.err().ok_or("expected error")?,
            "Command timed out after 100ms"
        );
        Ok(())
    }

    #[test]
    fn test_run_command_success_with_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--command_timeout", "2s", "sleep", "0.1"]);
        let result = run_command(&args);
        assert_eq!(result?, 0);
        Ok(())
    }

    #[test]
    fn test_run_command_fail() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "false"]);
        let result = run_command(&args);
        assert_eq!(result?, 1);
        Ok(())
    }

    #[test]
    fn test_run_command_in_directory() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let file_path = temp_dir.path().join("foo.txt");
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(file_path)?;

        let args = Args::parse_from(vec![
            "argv0",
            "--directory",
            temp_dir.path().to_str().ok_or("invalid path")?,
            "test",
            "-f",
            "foo.txt",
        ]);
        let result = run_command(&args);
        assert_eq!(result?, 0);

        let args_fail = Args::parse_from(vec!["argv0", "test", "-f", "foo.txt"]);
        let result_fail = run_command(&args_fail);
        assert_eq!(result_fail?, 1);
        Ok(())
    }

    #[test]
    fn test_run_command_not_found() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "command_that_does_not_exist"]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("No such file or directory")
        );
        Ok(())
    }

    #[test]
    fn test_run_command_terminated_by_signal() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "bash", "-c", "kill $$"]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert_eq!(
            result.err().ok_or("expected error")?,
            "Command terminated by signal"
        );
        Ok(())
    }

    #[test]
    fn test_run_command_invalid_signal() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--command_timeout",
            "10ms",
            "--signal",
            "INVALID_SIGNAL",
            "sleep",
            "1",
        ]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("Invalid signal")
        );
        Ok(())
    }

    #[test]
    fn test_run_command_signal_timeout_kill() -> Result<(), Box<dyn std::error::Error>> {
        // This command ignores SIGTERM and sleeps for 2 seconds.
        // We set command_timeout to 100ms, so it will timeout.
        // We set signal to SIGTERM.
        // We set signal_timeout to 200ms.
        // It should receive SIGTERM, ignore it, wait 200ms, get SIGKILL, and die.
        let args = Args::parse_from(vec![
            "argv0",
            "--command_timeout",
            "100ms",
            "--signal",
            "SIGTERM",
            "--signal_timeout",
            "200ms",
            "sh",
            "-c",
            "trap '' TERM; sleep 2",
        ]);
        let start = Instant::now();
        let result = run_command(&args);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert_eq!(
            result.err().ok_or("expected error")?,
            "Command timed out after 100ms"
        );
        // Elapsed should be roughly 100ms (command timeout) + 200ms (signal timeout)
        // It should be at least 300ms.
        // allow some buffer for CI flakiness, but definitely > 100ms.
        assert!(elapsed >= Duration::from_millis(250));
        Ok(())
    }

    #[test]
    fn test_kill_child_process_group_killpg_error() -> Result<(), Box<dyn std::error::Error>> {
        let mut child = Command::new("true").spawn()?;
        child.wait()?; // Reap the child.

        let result = kill_child_process_group(&mut child, Some("SIGTERM"), Duration::from_secs(1));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ESRCH"));
        Ok(())
    }
}

#[cfg(test)]
mod lock_file {
    use super::*;
    use std::env;
    use std::thread;
    use tempfile::NamedTempFile;

    #[test]
    fn test_lock_file_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let mut temp_file = env::temp_dir();
        temp_file.push("test_lock_file_timeout.lock");

        let _lock = lock_file(&temp_file, Duration::from_millis(200))?;

        let lock_result = thread::spawn(move || lock_file(&temp_file, Duration::from_micros(500)))
            .join()
            .map_err(|_| "thread panicked")?;

        assert!(lock_result.is_err());
        assert!(
            lock_result
                .err()
                .ok_or("expected error")?
                .contains("Timeout waiting for lockfile after")
        );
        Ok(())
    }

    #[test]
    fn test_lock_file_error() -> Result<(), Box<dyn std::error::Error>> {
        let lock_result = lock_file(Path::new("/dev/fd"), Duration::from_secs(1));
        assert!(lock_result.is_err());
        assert!(
            lock_result
                .err()
                .ok_or("expected error")?
                .contains("Is a directory")
        );
        Ok(())
    }

    #[test]
    fn test_lock_file_retry() -> Result<(), Box<dyn std::error::Error>> {
        let temp_file = NamedTempFile::new()?;
        let lock_path = temp_file.path().to_owned();

        // Lock the file in the current thread first
        let lock1 = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock1.lock_exclusive()?;

        let start = Instant::now();
        let lock2 = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                drop(lock1);
            });

            let lock2 = lock_file(&lock_path, Duration::from_secs(2));
            assert!(start.elapsed() >= Duration::from_millis(500));
            lock2
        })?;
        drop(lock2);
        Ok(())
    }
}

#[cfg(test)]
mod make_command_to_run {
    use super::*;
    use mockito::Server;

    #[test]
    fn test_make_command_to_run_tmux() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name=my_window",
            "--lockfile=/tmp/foo.lock",
            "echo",
            "hello",
        ]);
        let result_args = make_command_to_run(args);
        let current_exe = env::current_exe()?.display().to_string();
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
        assert!(result_args.lock_timeout.is_none());
        assert!(result_args.command_timeout.is_none());
        assert!(result_args.directory.is_none());
        assert_eq!(Duration::from_secs(1), result_args.signal_timeout);
        assert!(!result_args.shell);
        assert!(result_args.network_check_timeout.is_none());
        assert_eq!(Duration::from_secs(1), result_args.url_retry_delay);
        assert_eq!(5, result_args.url_retry_count);
        Ok(())
    }

    #[test]
    fn test_make_command_to_run_network_check() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=500ms",
            "--network_check_url=http://example.com",
            "true",
        ]);
        let result_args = make_command_to_run(args);
        assert_eq!(
            result_args.network_check_timeout,
            Some(Duration::from_millis(500))
        );
        assert_eq!(result_args.network_check_url, "http://example.com");
        Ok(())
    }

    #[test]
    fn test_network_check_timeout() -> Result<(), Box<dyn std::error::Error>> {
        // Create a listener that accepts a connection but sends nothing, forcing a timeout
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{}", port);

        std::thread::spawn(move || {
            // Accept the connection and sleep to force the client to timeout
            if let Ok((_stream, _)) = listener.accept() {
                std::thread::sleep(Duration::from_secs(2));
            }
        });

        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=100ms",
            "--network_check_url",
            &url,
            "true",
        ]);

        let result = run_command(&args);
        assert!(result.is_err());
        // Verify it failed due to network check
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("Network check failed")
        );
        Ok(())
    }

    #[test]
    fn test_network_check_success() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let _m = server.mock("HEAD", "/").with_status(200).create();

        let url = server.url();
        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=500ms",
            "--network_check_url",
            &url,
            "true",
        ]);

        let result = run_command(&args);
        assert_eq!(result?, 0);
        Ok(())
    }

    #[test]
    fn test_network_check_failure() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let _m = server.mock("HEAD", "/").with_status(500).create();

        let url = server.url();
        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=500ms",
            "--network_check_url",
            &url,
            "true",
        ]);

        let result = run_command(&args);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("Network check failed")
        );
        Ok(())
    }

    #[test]
    fn test_network_check_dns_failure() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=500ms",
            "--network_check_url=http://invalid.domain.that.does.not.exist.example.com",
            "true",
        ]);
        let result = run_command(&args);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .ok_or("expected error")?
                .contains("Network check failed")
        );
        Ok(())
    }

    #[test]
    fn test_network_check_timeout_zero() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = Server::new();
        let _m = server.mock("HEAD", "/").with_status(200).create();

        let url = server.url();
        let args = Args::parse_from(vec![
            "argv0",
            "--network_check_timeout=0s",
            "--network_check_url",
            &url,
            "true",
        ]);

        let result = run_command(&args);
        assert_eq!(result?, 0);
        Ok(())
    }

    #[test]
    fn test_make_command_to_run_shell() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--shell", "echo", "foo", "bar"]);
        let result_args = make_command_to_run(args);
        assert_eq!(result_args.command, vec!["sh", "-c", "echo", "foo", "bar"]);
        assert!(result_args.shell);
        Ok(())
    }

    #[test]
    fn test_make_command_to_run_no_modification() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--lockfile=/tmp/foo.lock", "echo", "hello"]);
        let original_args = args.clone();
        let result_args = make_command_to_run(args);
        assert_eq!(result_args.command, original_args.command);
        assert_eq!(result_args.lockfile, original_args.lockfile);
        assert_eq!(result_args.lock_timeout, original_args.lock_timeout);
        assert_eq!(result_args.command_timeout, original_args.command_timeout);
        assert_eq!(result_args.directory, original_args.directory);
        assert_eq!(result_args.shell, original_args.shell);
        assert_eq!(result_args.signal_timeout, original_args.signal_timeout);
        assert_eq!(result_args.caffeinate, original_args.caffeinate);
        Ok(())
    }

    #[test]
    fn test_make_command_to_run_caffeinate() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--caffeinate", "echo", "foo"]);
        let result_args = make_command_to_run(args);
        let mut expected_command = CAFFEINATE_CMD
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        expected_command.extend_from_slice(&["echo".to_string(), "foo".to_string()]);
        assert_eq!(result_args.command, expected_command);
        assert!(result_args.caffeinate);
        Ok(())
    }

    #[test]
    fn test_make_command_to_run_shell_caffeinate() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--shell", "--caffeinate", "echo", "foo"]);
        let result_args = make_command_to_run(args);
        let mut expected_command = CAFFEINATE_CMD
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        expected_command.extend_from_slice(&[
            "sh".to_string(),
            "-c".to_string(),
            "echo".to_string(),
            "foo".to_string(),
        ]);
        assert_eq!(result_args.command, expected_command);
        assert!(result_args.caffeinate);
        assert!(result_args.shell);
        Ok(())
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
    fn parse_args() -> Result<(), Box<dyn std::error::Error>> {
        // Checks that I've configured the parser correctly.
        let args = Args::parse_from(vec!["argv0", "echo"]);
        assert_eq!(vec!["echo".to_string()], args.command);
        assert!(!args.shell);
        assert_eq!(Duration::from_secs(1), args.url_retry_delay);
        assert_eq!(5, args.url_retry_count);

        let args = Args::parse_from(vec![
            "argv0",
            "--tmux_window_name",
            "asdf",
            "--lockfile",
            "qwerty",
            "--lock_timeout",
            "123ms",
            "--command_timeout",
            "456ms",
            "--signal",
            "SIGTERM",
            "--signal_timeout",
            "789ms",
            "--directory",
            "/no/where",
            "--shell",
            "--url_retry_delay",
            "2s",
            "--url_retry_count",
            "10",
            "echo",
            "foo",
            "bar",
        ]);
        assert_eq!(Some("asdf"), args.tmux_window_name.as_deref());
        assert_eq!(Some("qwerty"), args.lockfile.as_deref());
        assert_eq!(Some(Duration::from_millis(123)), args.lock_timeout);
        assert_eq!(Some(Duration::from_millis(456)), args.command_timeout);
        assert_eq!(Some("SIGTERM"), args.signal.as_deref());
        assert_eq!(Duration::from_millis(789), args.signal_timeout);
        assert_eq!(Some("/no/where"), args.directory.as_deref());
        assert_eq!(Duration::from_secs(2), args.url_retry_delay);
        assert_eq!(10, args.url_retry_count);
        assert_eq!(
            vec!["echo".to_string(), "foo".to_string(), "bar".to_string()],
            args.command
        );
        Ok(())
    }

    #[test]
    fn parse_args_wait() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--wait", "echo"]);
        assert!(args.wait);
        assert_eq!(vec!["echo".to_string()], args.command);
        Ok(())
    }

    #[test]
    fn test_parse_args_default_signal() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse_from(vec!["argv0", "--command_timeout", "100ms", "echo", "foo"]);
        assert_eq!(Some("SIGTERM"), args.signal.as_deref());
        assert_eq!(Duration::from_secs(1), args.signal_timeout);
        Ok(())
    }

    #[test]
    fn test_lock_timeout_requires_lockfile() -> Result<(), Box<dyn std::error::Error>> {
        let result = Args::try_parse_from(vec!["argv0", "--lock_timeout=100ms", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.err().ok_or("expected error")?;
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--lockfile"));
        Ok(())
    }

    #[test]
    fn test_signal_requires_command_timeout_ms() -> Result<(), Box<dyn std::error::Error>> {
        let result = Args::try_parse_from(vec!["argv0", "--signal=SIGTERM", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.err().ok_or("expected error")?;
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--command_timeout"));
        Ok(())
    }

    #[test]
    fn test_signal_timeout_requires_command_timeout_ms() -> Result<(), Box<dyn std::error::Error>> {
        let result = Args::try_parse_from(vec!["argv0", "--signal_timeout=100ms", "echo", "foo"]);
        assert!(result.is_err());
        let err = result.err().ok_or("expected error")?;
        let error_msg = err.to_string();
        assert!(error_msg.contains("required"));
        assert!(error_msg.contains("--command_timeout"));
        Ok(())
    }

    #[test]
    fn test_invalid_lock_timeout_format() {
        let result = Args::try_parse_from(vec![
            "argv0",
            "--lockfile=foo.lock",
            "--lock_timeout=invalid",
            "echo",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_command_timeout_format() {
        let result = Args::try_parse_from(vec!["argv0", "--command_timeout=invalid", "echo"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_signal_timeout_format() {
        let result = Args::try_parse_from(vec![
            "argv0",
            "--command_timeout=1s",
            "--signal_timeout=invalid",
            "echo",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_url_retry_delay_format() {
        let result = Args::try_parse_from(vec!["argv0", "--url_retry_delay=invalid", "echo"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_network_check_timeout_format() {
        let result = Args::try_parse_from(vec!["argv0", "--network_check_timeout=invalid", "echo"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_args_default() {
        let default_args = Args::default();
        assert!(!default_args.wait);
        assert!(default_args.tmux_window_name.is_none());
        assert!(default_args.lockfile.is_none());
        assert!(default_args.lock_timeout.is_none());
        assert!(default_args.command_timeout.is_none());
        assert!(default_args.signal.is_none());
        assert_eq!(default_args.signal_timeout, Duration::from_secs(1));
        assert!(default_args.directory.is_none());
        assert!(!default_args.shell);
        assert!(default_args.success_url.is_none());
        assert!(default_args.failure_url.is_none());
        assert_eq!(default_args.url_retry_delay, Duration::from_secs(1));
        assert_eq!(default_args.url_retry_count, 5);
        assert!(!default_args.caffeinate);
        assert!(default_args.network_check_timeout.is_none());
        assert_eq!(
            default_args.network_check_url,
            "http://clients3.google.com/generate_204"
        );
        assert!(default_args.output_shell_completion.is_none());
        assert!(default_args.command.is_empty());
    }
}

#[cfg(test)]
mod push_opt_tests {
    use super::*;

    #[test]
    fn test_push_opt_some_string() {
        let mut vec = Vec::new();
        push_opt(&mut vec, "--flag", Some("value".to_string()));
        assert_eq!(vec, vec!["--flag", "value"]);
    }

    #[test]
    fn test_push_opt_some_u64() {
        let mut vec = Vec::new();
        push_opt(&mut vec, "--timeout", Some(1000u64));
        assert_eq!(vec, vec!["--timeout", "1000"]);
    }

    #[test]
    fn test_push_opt_none() {
        let mut vec = Vec::new();
        let opt: Option<String> = None;
        push_opt(&mut vec, "--flag", opt);
        assert!(vec.is_empty());
    }
}

#[cfg(test)]
mod duration_parsing_tests {
    use super::*;

    #[test]
    fn test_parse_duration_valid() {
        assert_eq!(parse_duration("1s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("2h"), Ok(Duration::from_secs(7200)));
        assert_eq!(parse_duration("0"), Ok(Duration::from_secs(0)));
        assert_eq!(parse_duration("0s"), Ok(Duration::from_secs(0)));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("invalid").is_err());
        assert!(parse_duration("1s2ms").is_err());
        assert!(parse_duration("-5s").is_err());
    }

    #[test]
    fn test_lock_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let temp_dir = tempfile::tempdir().unwrap();
        let lock_path = temp_dir.path().join("test.lock");
        let _file = lock_file(&lock_path, Duration::from_millis(100)).unwrap();
        let metadata = std::fs::metadata(&lock_path).unwrap();
        let mode = metadata.permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "Lockfile permissions should be 0o600, but were 0o{:o}", mode & 0o777);
    }
}
