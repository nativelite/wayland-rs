#!/usr/bin/env python3
"""Local dev runner for wayland-rs — stdlib Python driving cargo, no task runner.

The same `python dev.py check` gate as every nativelite package, so the muscle
memory is identical across languages. Here `check` is the zero-dependency guard
plus `cargo fmt --check` and `cargo test` (unit + integration + doctests):

  python dev.py check                 # guard + cargo fmt --check + cargo test
                                      # (on Windows, then the same inside WSL:
                                      # the connection only builds on Linux)
  python dev.py linux                 # just the WSL half, from Windows
  python dev.py test                  # cargo test
  python dev.py build                 # cargo build --release
  python dev.py fmt                   # cargo fmt --check
  python dev.py guard                 # zero-dependency guard
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PY = sys.executable


def run(*args: str) -> int:
    print(f"$ {' '.join(args)}")
    return subprocess.call(args, cwd=str(ROOT))


def test() -> int:
    return run("cargo", "test", "--all-targets") or run("cargo", "test", "--doc")


def build() -> int:
    return run("cargo", "build", "--release")


def fmt() -> int:
    return run("cargo", "fmt", "--check")


def guard() -> int:
    return run(PY, "tools/dep_guard.py")


def linux() -> int:
    """The same gate inside WSL. On Windows the connection and the generated
    protocols are compiled out, so `check` there is only half the gate."""
    if sys.platform != "win32":
        return 0
    if shutil.which("wsl.exe") is None:
        print("WARNING: no WSL here, so the connection (src/conn, src/protocol,")
        print("tests/demo.rs, tests/live.rs) was NOT built or tested. Run the gate on Linux.")
        return 0
    drive, rest = ROOT.as_posix().split(":", 1)
    path = f"/mnt/{drive.lower()}{rest}"
    script = (
        f"cd '{path}' && CARGO_TARGET_DIR=$HOME/.cache/nl-target/{ROOT.name} "
        "python3 dev.py check"
    )
    return run("wsl.exe", "--", "bash", "-lc", script)


def check() -> int:
    return guard() or fmt() or test() or linux()


COMMANDS = {"test": test, "build": build, "fmt": fmt, "guard": guard, "linux": linux, "check": check}


def main(argv: list[str]) -> int:
    cmd = argv[1] if len(argv) > 1 else "check"
    fn = COMMANDS.get(cmd)
    if fn is None:
        print(f"unknown command {cmd!r}; choose from: {', '.join(COMMANDS)}")
        return 2
    return fn()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
