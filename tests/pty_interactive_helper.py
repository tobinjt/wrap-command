#!/usr/bin/env python3
"""Helper script to test wrap-command with interactive programs in a PTY."""

import os
import pty
import sys
import threading
import time


def run_child() -> None:
    import termios

    termios.tcsetattr(0, termios.TCSADRAIN, termios.tcgetattr(0))
    b = os.read(0, 5)
    if b.startswith(b"PING"):
        sys.exit(0)
    sys.exit(1)


def main() -> None:
    if len(sys.argv) >= 2 and sys.argv[1] == "--child":
        run_child()
        return

    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <path_to_wrap_command>", file=sys.stderr)
        sys.exit(2)

    wrap_cmd = sys.argv[1]

    pid, fd = pty.fork()
    if pid == 0:
        os.execlp(
            wrap_cmd,
            wrap_cmd,
            "--",
            sys.executable,
            os.path.abspath(__file__),
            "--child",
        )
    else:
        output: list[bytes] = []

        def reader() -> None:
            while True:
                try:
                    data = os.read(fd, 1024)
                    if not data:
                        break
                    output.append(data)
                except OSError:
                    break

        t = threading.Thread(target=reader)
        t.start()

        time.sleep(0.3)
        _ = os.write(fd, b"PING\n")

        _, status = os.waitpid(pid, 0)
        t.join(timeout=1)
        os.close(fd)
        sys.exit(os.waitstatus_to_exitcode(status))


if __name__ == "__main__":
    main()
