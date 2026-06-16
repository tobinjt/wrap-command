# wrap-command

Wrap a command with any combination of tmux, locking, and timeout.

## Timeout / Duration Formats

Any options accepting a timeout or duration (e.g., `--lock_timeout`,
`--command_timeout`, `--signal_timeout`, `--url_retry_delay`,
`--network_check_timeout`) accept a number followed by an optional time unit:

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
  - Multiple units (e.g., `1s2ms`) are **not** supported.
  - Negative values (e.g., `-5s`) are **not** supported.
