#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Compile the production Work protocol against minimal UART/clock/PWM stubs."""
import os
import shlex
import subprocess
import tempfile
from pathlib import Path

TEST = Path(__file__).resolve().parent
LIBRARY = TEST.parent


def main():
    with tempfile.TemporaryDirectory(prefix="picoharness-work-regression-") as directory:
        build = Path(directory)
        for header in ("hardware/clocks.h", "hardware/pwm.h", "hardware/uart.h", "pico/stdlib.h"):
            path = build / header
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('#include "fake_sdk.h"\n')
        executable = build / "regression"
        compiler = shlex.split(os.environ.get("CC", "cc"))
        subprocess.run(compiler + [
            "-std=c11", "-Wall", "-Wextra", "-Werror", "-g",
            "-I", str(build), "-I", str(TEST), "-I", str(LIBRARY / "include"),
            str(TEST / "regression.c"), str(LIBRARY / "src/pico_harness.c"),
            "-o", str(executable),
        ], check=True)
        subprocess.run([str(executable)], check=True)


if __name__ == "__main__":
    main()
