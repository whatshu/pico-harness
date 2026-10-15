"""Flash and test the dualcore RP2040 Work core through a Debug probe."""

import argparse
import os
from pathlib import Path
import socket
import subprocess
import sys
import time

from .device import Probe
from .protocol import Command, decode_status
from .update import is_dualcore_probe, probe_core, uf2_family


def validate_images(elf, flash_elf, uf2):
    if uf2_family(uf2) != 0xE48BFF56:
        raise RuntimeError(f"{uf2} does not target RP2040")
    for image, start, end in ((flash_elf, 0x10000000, 0x10200000),
                              (elf, 0x20000000, 0x20042000)):
        attributes = subprocess.check_output(
            ["arm-none-eabi-readelf", "-A", image], text=True)
        if "Tag_CPU_arch: v6S-M\n" not in attributes:
            raise RuntimeError(f"{image} is not an RP2040 Cortex-M0+ ELF")
        symbols = subprocess.check_output(
            ["arm-none-eabi-nm", image], text=True)
        entry = next((int(fields[0], 16) for line in symbols.splitlines()
                      if len(fields := line.split()) == 3 and fields[2] == "_entry_point"), None)
        if entry is None or not start <= entry < end:
            raise RuntimeError(f"{image} has the wrong execution address")
    print("Work RP2040 UF2, Flash ELF and RAM ELF verified", flush=True)


def connected_probe(serial, required):
    try:
        probe = Probe.open(serial)
    except RuntimeError as error:
        if required or serial:
            raise RuntimeError(f"Work hardware test requires a Debug probe: {error}") from error
        print(f"Work hardware test skipped: {error}", flush=True)
        return None
    try:
        info = Probe._ok(probe.command(Command.GET_INFO), "get info").rstrip(b"\0").decode()
    except Exception:
        probe.close()
        raise
    if not is_dualcore_probe(info):
        probe.close()
        if required or serial:
            raise RuntimeError(f"Work hardware test requires a dualcore Debug probe: {info}")
        print(f"Work hardware test skipped: connected probe is {info}", flush=True)
        return None
    if probe_core(info) != "pico":
        probe.close()
        raise RuntimeError(f"Work hardware test requires an RP2040 Debug core: {info}")
    return probe


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_for_openocd(process, port, log_path):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.1)
    tail = log_path.read_text(errors="replace")[-2000:]
    raise RuntimeError(f"OpenOCD Tcl server did not start; see {log_path}\n{tail}")


def run(elf, flash_elf, uf2, output, script, serial=None, required=False):
    validate_images(elf, flash_elf, uf2)
    probe = connected_probe(serial, required)
    if probe is None:
        return
    serial = probe.serial
    try:
        status = decode_status(probe.command(Command.GET_STATUS))
    except Exception:
        probe.close()
        raise
    was_powered = status["power_on"]
    process = None
    try:
        if not was_powered:
            Probe._ok(probe.command(Command.POWER_SET, b"\x01"), "power on Work")
        status = decode_status(probe.command(Command.GET_STATUS))
        if not status["power_good"]:
            raise RuntimeError(f"Work power is not good: {status}")
        probe.close()

        output.parent.mkdir(parents=True, exist_ok=True)
        log_path = output.with_suffix(".openocd.log")
        port = free_port()
        env = os.environ.copy()
        env["PDBG_PROBE_SERIAL"] = serial
        with log_path.open("w") as log:
            process = subprocess.Popen([
                str(script), "-c", f"tcl_port {port}",
                "-c", "gdb_port disabled", "-c", "telnet_port disabled",
                "-c", "init",
            ], stdout=log, stderr=subprocess.STDOUT, env=env)
            try:
                wait_for_openocd(process, port, log_path)
                subprocess.run([
                    sys.executable, "-m", "pdbg.worktest",
                    "--elf", str(elf), "--flash-elf", str(flash_elf), "--serial", serial,
                    "--port", str(port), "--output", str(output),
                ], check=True)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    finally:
        probe.close()
        restore = Probe.open(serial)
        try:
            if was_powered:
                Probe._ok(restore.command(Command.RESET_PULSE, b"\x0a\x00"),
                          "restart Work flash image")
            else:
                Probe._ok(restore.command(Command.POWER_SET, b"\x00"), "restore Work power")
        finally:
            restore.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", required=True, type=Path)
    parser.add_argument("--flash-elf", required=True, type=Path)
    parser.add_argument("--uf2", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--openocd-script", type=Path)
    parser.add_argument("--serial")
    parser.add_argument("--require-hardware", action="store_true")
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    try:
        if args.validate_only:
            validate_images(args.elf, args.flash_elf, args.uf2)
        else:
            if args.openocd_script is None:
                parser.error("--openocd-script is required for hardware testing")
            run(args.elf, args.flash_elf, args.uf2, args.output, args.openocd_script,
                args.serial, args.require_hardware)
    except RuntimeError as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
