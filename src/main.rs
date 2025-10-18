use clap::Parser;

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

    /// The timeout in seconds.
    #[arg(long = "timeout")]
    timeout: Option<u64>,

    /// The command to run.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

fn main() {
    let args = Args::parse();

    println!("tmux_window_name: {:?}", args.tmux_window_name);
    println!("lockfile: {:?}", args.lockfile);
    println!("timeout: {:?}", args.timeout);
    println!("command: {:?}", args.command);
}
