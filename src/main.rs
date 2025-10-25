use clap::Parser;
use fs4::fs_std::FileExt;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

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

fn realmain(args: Args) {
    println!("tmux_window_name: {:?}", args.tmux_window_name);
    println!("lockfile: {:?}", args.lockfile);
    println!("lock_timeout: {:?}", args.lock_timeout);
    println!("command_timeout: {:?}", args.command_timeout);
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
            "echo",
            "foo",
        ]));
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
