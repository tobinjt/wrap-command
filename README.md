# wrap-command

`wrap-command` is a versatile command-line utility that wraps the execution of
any command with a combination of common operations like file locking, timeouts,
automatic retries, sleep prevention, network checks, tmux integration, and
webhook notifications.

It's designed to be a robust helper for cron jobs, automation scripts, and
background tasks.

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Installation](#installation)
- [Features & Examples of Usage](#features--examples-of-usage)
  - [1. File Locking (`--lockfile`)](#1-file-locking---lockfile)
  - [2. Timeouts & Signal Control (`--command_timeout`)](#2-timeouts--signal-control---command_timeout)
  - [3. Automatic Retries (`--retries`)](#3-automatic-retries---retries)
  - [4. Preventing System Sleep (`--caffeinate`)](#4-preventing-system-sleep---caffeinate)
  - [5. Notifications & Webhook Pings (`--success_url` / `--failure_url`)](#5-notifications--webhook-pings---success_url----failure_url)
  - [6. Waiting for Network Connectivity (`--network_check_timeout`)](#6-waiting-for-network-connectivity---network_check_timeout)
  - [7. Execution Environment & Interaction](#7-execution-environment--interaction)
- [Order of Execution](#order-of-execution)
  - [Lifecycle Rules](#lifecycle-rules)
- [Timeout / Duration Formats](#timeout--duration-formats)
- [Shell Autocompletion](#shell-autocompletion)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Installation

`wrap-command` is written in Rust and needs a Rust installation to compile it.
See <https://www.rust-lang.org/tools/install> for how to install Rust.

Once Rust is installed, `wrap-command` can be installed from
<https://crates.io/crates/wrap-command>:

```shell
cargo install wrap-command
```

## Features & Examples of Usage

Any combination of features is supported. Some flags require other flags, e.g.
`--lock_timeout` is rejected if `--lockfile` isn't used.

### 1. File Locking (`--lockfile`)

Ensure only one instance of a command runs at any given time.

- `--lockfile <PATH>`: Path to the lockfile.
- `--lock_timeout <DURATION>`: Fails immediately if the lock is held (default),
  or waits up to the specified duration for the lock to be released.

**Example:** Run a backup script, waiting up to 5 minutes to acquire the lock if
another instance is already running:

```shell
wrap-command --lockfile /tmp/backup.lock --lock_timeout 5m rsync -avz /data /backup
```

### 2. Timeouts & Signal Control (`--command_timeout`)

Enforce an execution time limit on your command.

- `--command_timeout <DURATION>`: Kill the command if it runs for longer than
  this.
- `--signal <SIGNAL>`: The signal to send to the command on timeout. Can be a
  signal name (e.g., `SIGTERM`, `SIGINT`) or number (e.g., `15`). Defaults to
  `SIGTERM`.
- `--signal_timeout <DURATION>`: The time to wait for the child process to exit
  after sending the signal. If it doesn't exit, it's forcefully killed with
  `SIGKILL` (9). Defaults to `1s`.

**Example:** Kill a test runner if it takes longer than 30 seconds, sending
`SIGINT` first and waiting up to 5 seconds before resorting to `SIGKILL`:

```shell
wrap-command --command_timeout 30s --signal SIGINT --signal_timeout 5s cargo test
```

### 3. Automatic Retries (`--retries`)

Retry a command if it fails (returns a non-zero exit code).

- `--retries <COUNT>`: The number of times to retry.
- `--retry_delay <DURATION>`: How long to wait between retries (can't be used
  with `--retry_wait`).
- `--retry_wait`: Wait for the user to press Enter before retrying (can't be
  used with `--retry_delay`).

**Example:** Try fetching a remote resource up to 3 times, waiting 10 seconds
between attempts:

```shell
wrap-command --retries 3 --retry_delay 10s curl -s -f https://example.com/api
```

### 4. Preventing System Sleep (`--caffeinate`)

Keep the system awake while your command runs by prepending an OS-specific
command to the command being run.

- `--caffeinate`: Prevents the system from sleeping or idling.
  - On **MacOS**, uses `caffeinate -i`.
  - On **Linux**, uses `systemd-inhibit --what=idle`.

**Example:** Run a heavy compilation or machine learning training job without
the system going to sleep:

```shell
wrap-command --caffeinate python train.py
```

### 5. Notifications & Webhook Pings (`--success_url` / `--failure_url`)

Notify external services (like Healthchecks.io, Dead Man's Snitch, etc.) when
your command succeeds or fails.

- `--success_url <URL>`: Ping this URL on successful execution (exit code 0).
- `--failure_url <URL>`: Ping this URL on command failure (non-zero exit code).
- `--url_retry_count <COUNT>`: Number of retries when pinging URLs (defaults to
  5).
- `--url_retry_delay <DURATION>`: Delay between URL ping retries (defaults to
  1s).

**Example:** Track cron job success with a health check service:

```shell
wrap-command --success_url "https://hc-ping.com/uuid" \
  --failure_url "https://hc-ping.com/uuid/fail" \
  -- my-cron-script.sh
```

### 6. Waiting for Network Connectivity (`--network_check_timeout`)

Ensure network access is available before attempting to run the command.

- `--network_check_timeout <DURATION>`: Max duration to wait for network
  availability. Set to `0s` to wait indefinitely. No network check is performed
  if this flag isn't used.
- `--network_check_url <URL>`: Custom URL to check connectivity (defaults to
  `http://clients3.google.com/generate_204`).

If the network check fails `wrap-command` exits unsuccessfully. If
`--failure_url` is used with `--network_check_timeout`, and the network check
fails, the failure URL **won't** be pinged because the network is unavailable.

**Example:** Wait for up to 1 minute for a network connection before initiating
a package update:

```shell
wrap-command --network_check_timeout 1m -- sudo apt-get update
```

### 7. Execution Environment & Interaction

- `--directory <PATH>`: Run the command from a different working directory.
  `wrap-command` itself doesn't change directory, it's just changed for the
  child process, so using --directory doesn't affect the location of the lock
  file. When retries are used the directory is changed for **each** new child
  process, so if the directory is changed, e.g. by replacing it with a symlink,
  a later child process would run in a different directory.

- `--shell`: Prepend `"sh" "-c"` to the command so you can use shell features
  like pipelines or environment variables. Doesn't otherwise change the command,
  in particular if you use `--shell` with multiple arguments they will be passed
  as separate arguments to `sh`. E.g.
  `"wrap-command" "--shell" "ls" "foo" "bar"` results in:

  ```shell
  "sh" "-c" "ls" "foo" "bar"
  ```

  *not*:

  ```shell
  "sh" "-c" "ls foo bar"
  ```

- `--tmux_window_name <NAME>`: Run the command in a new tmux window with the
  specified name. This is implemented by running:

  ```shell
  tmux new-window -d -n <window_name> \
    wrap-command <reconstructed_flags> <target_command>
  ```

- `--wait`: Wait for the user to press Enter after the command has finished
  (great for inspecting output before closing a terminal window).

**Example:** Run build commands in a specific project folder, inside tmux, and
keep the shell open afterwards:

```shell
wrap-command --directory /path/to/project \
  --tmux_window_name "build" --wait \
  cargo build --release
```

## Order of Execution

The order of execution is consistent regardless of the flags used, but
**whether** a step is executed depends on the flags that were specified.

1. **tmux delegation** (`--tmux_window_name`): Spawns a new tmux window running
   a recursive `wrap-command` call with the other arguments and exits
   immediately. All remaining steps below execute inside that tmux window.
1. **Lock acquisition** (`--lockfile` & `--lock_timeout`): The tool tries to
   acquire the file lock. If another process holds it, it waits up to the
   timeout.
1. **Network connectivity check** (`--network_check_timeout` &
   `--network_check_url`): Once the lock is acquired, the tool verifies
   connectivity.
1. **Command execution loop**:
   - The system-sleep prevention command (caffeinate/systemd-inhibit) is wrapped
     around the target command if `--caffeinate` is enabled.
   - The shell wrapper (`sh -c`) is prepended if `--shell` is enabled.
   - The command is executed, optionally in a different directory
     (`--directory`). If a timeout (`--command_timeout`) occurs, the configured
     signal (`--signal`) is sent to the process group; after waiting up to
     `--signal_timeout` for the process group to exit, `SIGKILL` is sent.
   - If the command fails, the tool waits for the specified delay
     (`--retry_delay`) or for the user to press Enter (`--retry_wait`) and
     retries the command up to `--retries` times.
1. **Webhook notifications** (`--success_url` & `--failure_url`): Pings the
   corresponding webhook based on the final exit code.
1. **User acknowledgement** (`--wait`): Waits for the user to press Enter.
1. **Exit & Lock Release**: The process exits, automatically releasing the file
   lock.

### Lifecycle Rules

- **Locking & Network Checks**: The lockfile is acquired **before** the network
  connectivity check. This ensures that only one process is performing the
  network check and waiting on connectivity at a time.
- **Locking & Retries**: The lock is **held continuously** across all command
  retries and delays. It's not released and re-acquired between retries; it's
  only released when `wrap-command` exits.
- **Network Checks & Retries**: The network connectivity check is performed
  **only once** at the beginning. It's **not** repeated on each command retry.

## Timeout / Duration Formats

Any options accepting a timeout or duration (e.g., `--lock_timeout`) accept a
number followed by an optional time unit:

- **Supported units**:
  - `ns` (nanoseconds)
  - `us` or `µs` (microseconds)
  - `ms` (milliseconds)
  - `s` (seconds)
  - `m` (minutes)
  - `h` (hours)
  - `d` (days)
  - `w` (weeks)
  - `M` (months)
  - `y` (years)
- **Defaults**: If no unit is specified, seconds (`s`) is assumed (e.g., `10` is
  parsed as 10 seconds).
- **Floating-point**: Standard floating-point values and scientific notations
  are supported (e.g., `1.5s`, `.5h`, `2e-3s`).
- **Restrictions**:
  - Mixed units (e.g., `1s2ms`) are **not** supported.
  - Negative values (e.g., `-5s`) are **not** supported.

## Shell Autocompletion

`wrap-command` can generate autocompletion scripts for shells using the
`--output_shell_completion` flag.

Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

**Example:** Generate and load autocomplete definitions for Zsh:

```shell
wrap-command --output_shell_completion zsh > ~/.zsh/completion/_wrap-command
```

## License

Licensed under the Apache 2.0 license, see the [`LICENSE`](LICENSE) file
accompanying the software.
