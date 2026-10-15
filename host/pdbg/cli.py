"""Command-line board automation client."""

import argparse
from datetime import datetime, timezone
import json
import struct
import sys
import time
from .device import Probe
from .logic import DIRECT_GPIO_PINS, analyze_direct_pattern
from .protocol import (
    Command,
    Measurement,
    SPI_TEST_BLOCK_SIZE,
    SpiSelftestReport,
    spi_test_pattern,
)


def require_ok(payload, action):
    """Return a successful response or make protocol failures visible to CI."""
    if not payload:
        raise RuntimeError(f"{action} failed: empty response")
    if payload[0]:
        raise RuntimeError(f"{action} failed with status {payload[0]}")
    return payload


def main(argv=None):
    parser = argparse.ArgumentParser(prog="pdbgctl")
    parser.add_argument("--serial")
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("info")
    sub.add_parser("status")
    power = sub.add_parser("power")
    power.add_argument("state", choices=("on", "off"))
    reset = sub.add_parser("reset")
    reset.add_argument("--milliseconds", type=int, default=10)
    sub.add_parser("bootsel")
    update = sub.add_parser("update")
    update.add_argument("uf2", help="UF2 to load after the probe enters ROM BOOTSEL")
    update.add_argument("--picotool", default="picotool", help="picotool executable")
    flash = sub.add_parser("flash")
    flash.add_argument("--artifacts", required=True)
    flash.add_argument("--picotool", default="picotool")
    monitor = sub.add_parser("monitor")
    monitor.add_argument("--interval", type=float, default=1.0)
    sub.add_parser("measure")
    sub.add_parser("gpio")
    gpio_diag = sub.add_parser("gpio-diag")
    gpio_diag.add_argument("pin", type=int)
    gpio_pull = sub.add_parser("gpio-pull")
    gpio_pull.add_argument("pin", type=int)
    gpio_pull.add_argument("mode", choices=("none", "down", "up"))
    capture = sub.add_parser("capture")
    capture.add_argument("--samples", type=int, default=1024)
    capture.add_argument("--frequency", type=int, default=1_000_000)
    capture.add_argument("--output", required=True, help="CSV output path")
    logic_sweep = sub.add_parser("logic-sweep")
    logic_sweep.add_argument(
        "--rates",
        default="10000000,20000000,30000000,40000000,50000000,60000000,"
                "75000000,100000000,125000000,150000000",
        help="comma-separated requested GPIO sample rates",
    )
    logic_sweep.add_argument("--samples", type=int, default=32768)
    logic_sweep.add_argument("--duration", type=float, default=0,
                             help="test each rate for at least this many seconds")
    logic_sweep.add_argument("--pattern-frequency", type=int, default=1_000_000)
    logic_sweep.add_argument("--no-reset", action="store_true")
    uart_config = sub.add_parser("uart-config")
    uart_config.add_argument("channel", type=int, choices=(0, 1))
    uart_config.add_argument("baud", type=int)
    uart_config.add_argument("--data-bits", type=int, choices=range(5, 9), default=8)
    uart_config.add_argument("--stop-bits", type=int, choices=(1, 2), default=1)
    uart_config.add_argument("--parity", choices=("none", "odd", "even"), default="none")
    uart_write = sub.add_parser("uart-write")
    uart_write.add_argument("channel", type=int, choices=(0, 1))
    uart_write.add_argument("data")
    uart_write.add_argument("--hex", action="store_true", help="interpret data as hexadecimal bytes")
    uart_read = sub.add_parser("uart-read")
    uart_read.add_argument("channel", type=int, choices=(0, 1))
    uart_read.add_argument("--maximum", type=int, default=4095)
    uart_read.add_argument("--hex", action="store_true", help="print hexadecimal bytes")
    uart_status = sub.add_parser("uart-status")
    uart_status.add_argument("channel", type=int, choices=(0, 1))
    spi_config = sub.add_parser("spi-config")
    spi_config.add_argument("baud", type=int)
    spi_config.add_argument("--mode", type=int, choices=range(4), default=0)
    spi_transfer = sub.add_parser("spi-transfer")
    spi_transfer.add_argument("data", help="hexadecimal bytes to transfer")
    spi_sweep = sub.add_parser("spi-sweep")
    spi_sweep.add_argument(
        "--rates",
        default="1000000,2000000,4000000,8000000,12000000,16000000,"
                "20000000,24000000,30000000,40000000,50000000,60000000",
        help="comma-separated requested SPI bit rates",
    )
    spi_sweep.add_argument("--blocks", type=int, default=20)
    spi_sweep.add_argument("--duration", type=float, default=0,
                           help="test each rate for at least this many seconds")
    spi_sweep.add_argument("--no-reset", action="store_true")
    args = parser.parse_args(argv)
    try:
        probe = Probe.open(args.serial)
    except RuntimeError as error:
        probe = None
        probe_error = error

    if args.action == "update":
        from .update import update_debug_core
        if probe is None:
            print(f"PDBG probe not found ({probe_error}); checking for an "
                  "existing ROM BOOTSEL device", file=sys.stderr)
        update_debug_core(probe, args.uf2, picotool=args.picotool)
        return

    if probe is None:
        raise SystemExit(f"PDBG probe open failed: {probe_error}")

    if args.action == "flash":
        from .update import debug_image_for_probe, update_debug_core
        image = debug_image_for_probe(probe, args.artifacts)
        update_debug_core(probe, image, picotool=args.picotool)
        return

    if args.action == "monitor":
        if args.interval <= 0:
            parser.error("--interval must be positive")
        from .protocol import decode_status
        from .update import is_dualcore_probe, probe_core
        info = require_ok(probe.command(Command.GET_INFO), "get info")[1:].rstrip(b"\0").decode()
        if not is_dualcore_probe(info):
            raise SystemExit(f"connected probe is not a dualcore Debug core: {info}")
        if probe_core(info) != "pico":
            raise SystemExit(f"connected Debug core is not RP2040: {info}")
        try:
            while True:
                print(json.dumps({
                    "time": datetime.now(timezone.utc).isoformat(),
                    "serial": probe.serial,
                    "status": decode_status(probe.command(Command.GET_STATUS)),
                    "measurement": Measurement.from_payload(
                        probe.command(Command.MEASURE)).__dict__,
                }), flush=True)
                time.sleep(args.interval)
        except KeyboardInterrupt:
            return

    if args.action == "info":
        payload = require_ok(probe.command(Command.GET_INFO), "get info")
        print(payload[1:].rstrip(b"\0").decode())
    elif args.action == "status":
        from .protocol import decode_status
        print(json.dumps(decode_status(probe.command(Command.GET_STATUS))))
    elif args.action == "power":
        require_ok(probe.command(Command.POWER_SET, bytes([args.state == "on"])), "set power")
    elif args.action == "reset":
        require_ok(
            probe.command(Command.RESET_PULSE, struct.pack("<H", args.milliseconds)),
            "pulse reset",
        )
    elif args.action == "bootsel":
        require_ok(probe.command(Command.ENTER_BOOTSEL), "enter BOOTSEL")
    elif args.action == "measure":
        print(json.dumps(Measurement.from_payload(probe.command(Command.MEASURE)).__dict__))
    elif args.action == "gpio":
        status, sample = struct.unpack(
            "<BI", require_ok(probe.command(Command.GPIO_READ), "read GPIO")
        )
        print(json.dumps({"status": status, "gpio": sample, "hex": f"0x{sample:08x}"}))
    elif args.action == "gpio-diag":
        status, value, output, function, pull_up, pull_down = require_ok(
            probe.command(Command.GPIO_DIAG, bytes([args.pin])),
            "diagnose GPIO",
        )
        print(json.dumps({"status": status, "pin": args.pin, "value": value, "output": bool(output),
                          "function": function, "pull_up": bool(pull_up), "pull_down": bool(pull_down)}))
    elif args.action == "gpio-pull":
        modes = {"none": 0, "down": 1, "up": 2}
        status, = require_ok(
            probe.command(Command.GPIO_PULL, bytes([args.pin, modes[args.mode]])),
            "set GPIO pull",
        )
        print(json.dumps({"status": status, "pin": args.pin, "pull": args.mode}))
    elif args.action == "capture":
        result = probe.capture(args.samples, args.frequency)
        with open(args.output, "w", encoding="utf-8") as output:
            output.write("sample,time_s,gpio_hex\n")
            for index, value in enumerate(result.samples):
                output.write(
                    f"{index},{index / result.actual_frequency:.9f},0x{value:08x}\n"
                )
        print(json.dumps({
            "capture_id": result.capture_id,
            "samples": result.count,
            "requested_hz": args.frequency,
            "actual_hz": round(result.actual_frequency),
            "elapsed_us": result.elapsed_us,
            "output": args.output,
        }))
    elif args.action == "logic-sweep":
        if not 2 <= args.samples <= 32768:
            parser.error("--samples must be in the range 2..32768")
        if args.duration < 0:
            parser.error("--duration must not be negative")
        if args.pattern_frequency <= 0:
            parser.error("--pattern-frequency must be positive")
        rates = [int(value) for value in args.rates.split(",")]
        if not rates or any(rate < 2500 for rate in rates):
            parser.error("--rates must contain integers of at least 2500 Hz")
        if not args.no_reset:
            probe.reset(10)
            time.sleep(2.0)

        highest_stable = None
        all_active_pins = set()
        for requested in rates:
            started = time.perf_counter()
            captures = 0
            total_samples = 0
            total_capture_us = 0
            transition_errors = 0
            invalid_samples = 0
            maximum_delta = 0
            expected_maximum_delta = 0
            minimum_actual = None
            missed_rate_captures = 0
            while True:
                result = probe.capture(args.samples, requested)
                analysis = analyze_direct_pattern(
                    result.samples, result.actual_frequency,
                    args.pattern_frequency,
                )
                captures += 1
                total_samples += result.count
                total_capture_us += result.elapsed_us
                transition_errors += analysis.transition_errors
                invalid_samples += analysis.invalid_samples
                maximum_delta = max(maximum_delta, analysis.maximum_delta)
                expected_maximum_delta = max(
                    expected_maximum_delta, analysis.expected_maximum_delta
                )
                minimum_actual = (
                    result.actual_frequency if minimum_actual is None
                    else min(minimum_actual, result.actual_frequency)
                )
                if result.actual_frequency < requested * 0.95:
                    missed_rate_captures += 1
                all_active_pins.update(
                    pin for pin in DIRECT_GPIO_PINS
                    if pin not in analysis.inactive_pins
                )
                if not args.duration or time.perf_counter() - started >= args.duration:
                    break

            actual = total_samples * 1_000_000 / total_capture_us
            stable = (transition_errors == 0 and invalid_samples == 0 and
                      missed_rate_captures == 0)
            if stable:
                highest_stable = max(highest_stable or 0, round(actual))
            print(json.dumps({
                "requested_hz": requested,
                "actual_hz": round(actual),
                "minimum_actual_hz": round(minimum_actual),
                "captures": captures,
                "samples": total_samples,
                "capture_elapsed_us": total_capture_us,
                "wall_elapsed_s": round(time.perf_counter() - started, 6),
                "transition_errors": transition_errors,
                "invalid_samples": invalid_samples,
                "maximum_delta": maximum_delta,
                "expected_maximum_delta": expected_maximum_delta,
                "rate_miss_captures": missed_rate_captures,
                "stable": stable,
            }))
        unvalidated = sorted(set(DIRECT_GPIO_PINS) - all_active_pins)
        print(json.dumps({
            "highest_stable_hz": highest_stable,
            "gpio_pins": list(DIRECT_GPIO_PINS),
            "unvalidated_pins": unvalidated,
            "gpio_valid": not unvalidated,
        }))
    elif args.action == "uart-config":
        parity = {"none": 0, "odd": 1, "even": 2}[args.parity]
        actual = probe.uart_config(args.channel, args.baud, args.data_bits,
                                   args.stop_bits, parity)
        print(json.dumps({"channel": args.channel, "requested_baud": args.baud,
                          "actual_baud": actual}))
    elif args.action == "uart-write":
        data = bytes.fromhex(args.data) if args.hex else args.data.encode()
        print(json.dumps({"channel": args.channel,
                          "written": probe.uart_write(args.channel, data)}))
    elif args.action == "uart-read":
        data = probe.uart_read(args.channel, args.maximum)
        if args.hex:
            print(data.hex())
        else:
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()
    elif args.action == "uart-status":
        status = probe.uart_status(args.channel)
        print(json.dumps({"channel": args.channel, **status.__dict__}))
    elif args.action == "spi-config":
        actual = probe.spi_config(args.baud, args.mode)
        print(json.dumps({"requested_baud": args.baud, "actual_baud": actual,
                          "mode": args.mode}))
    elif args.action == "spi-transfer":
        print(probe.spi_transfer(bytes.fromhex(args.data)).hex())
    elif args.action == "spi-sweep":
        if args.blocks <= 0:
            parser.error("--blocks must be positive")
        if args.duration < 0:
            parser.error("--duration must not be negative")
        rates = [int(value) for value in args.rates.split(",")]
        if not rates or any(rate <= 0 for rate in rates):
            parser.error("--rates must contain positive integers")
        pattern = spi_test_pattern(SPI_TEST_BLOCK_SIZE)
        highest_stable = None
        tested_actual = set()
        for requested in rates:
            if not args.no_reset:
                probe.reset(10)
                time.sleep(2.0)
            actual = probe.spi_config(requested, 0)
            if actual in tested_actual:
                continue
            tested_actual.add(actual)
            started = time.perf_counter()
            miso_errors = 0

            def transfer_report():
                nonlocal miso_errors
                try:
                    return SpiSelftestReport.from_transfer(
                        probe.spi_transfer(pattern, timeout=5000)
                    )
                except ValueError:
                    miso_errors += 1
                    return None

            baseline = transfer_report()
            tested_blocks = 0
            while (time.perf_counter() - started < args.duration
                   if args.duration else tested_blocks < args.blocks):
                transfer_report()
                tested_blocks += 1
            final = transfer_report()
            elapsed = time.perf_counter() - started
            completed = (final.blocks - baseline.blocks
                         if baseline is not None and final is not None else 0)
            mosi_errors = (final.total_errors - baseline.total_errors
                           if baseline is not None and final is not None else -1)
            stable = (miso_errors == 0 and mosi_errors == 0 and
                      completed >= tested_blocks + 1)
            if stable:
                highest_stable = actual
            print(json.dumps({
                "requested_hz": requested,
                "actual_hz": actual,
                "blocks": tested_blocks,
                "block_bytes": SPI_TEST_BLOCK_SIZE,
                "completed_delta": completed,
                "mosi_errors": mosi_errors,
                "miso_error_blocks": miso_errors,
                "elapsed_s": round(elapsed, 6),
                "stable": stable,
            }))
        print(json.dumps({"highest_stable_hz": highest_stable,
                          "tested_rates": len(tested_actual)}))


if __name__ == "__main__":
    main()
