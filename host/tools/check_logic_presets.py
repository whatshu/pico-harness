#!/usr/bin/env python3
"""Check RP2040 capture presets against a running harness-signals Work image.

This test does not program firmware or change Work power. It requires the
GP5 10 kHz fixture and an explicit probe serial. Run from the repository root:
python3 -m host.tools.check_logic_presets --serial SERIAL --output /tmp/logic.json
"""

import argparse
import json
import struct
import time
from pathlib import Path

from host.pdbg.device import Probe
from host.pdbg.protocol import Command

SAMPLE_RATES = (100_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000, 20_000_000)
RUNNING, STALL, TIMEOUT, CANCELLED = 16, 2, 4, 8


def start(probe, rate, edge, pin=5, timeout_ms=1000):
    payload = struct.pack("<IIBBHBB", 32768, rate, edge, pin, timeout_ms, 0, 0)
    reply = Probe._ok(probe.command(Command.CAPTURE_START, payload), "start capture")
    return struct.unpack("<I", reply)[0]


def poll(probe, capture_id):
    deadline = time.monotonic() + 3
    while True:
        reply = Probe._ok(probe.command(Command.CAPTURE_POLL), "poll capture")
        actual_id, rate, elapsed, flags, count = struct.unpack("<IIIII", reply)
        assert actual_id == capture_id, "Capture was replaced"
        if not flags & RUNNING:
            return rate, elapsed, flags, count
        if time.monotonic() > deadline:
            raise TimeoutError("Capture did not complete within 3 seconds")
        time.sleep(0.002)


def read(probe, capture_id, count):
    words = []
    for offset in range(0, count, 1023):
        length = min(1023, count - offset)
        payload = struct.pack("<IIH", capture_id, offset, length)
        reply = Probe._ok(probe.command(Command.CAPTURE_READ, payload), "read capture")
        words.extend(struct.unpack(f"<{length}I", reply))
    return words


def check_pwm(probe, rate, edge):
    capture_id = start(probe, rate, edge)
    actual, elapsed, flags, count = poll(probe, capture_id)
    assert not flags & (STALL | TIMEOUT | CANCELLED), f"Capture flags {flags:#x}"
    assert count == 32768
    levels = [bool(word & (1 << 5)) for word in read(probe, capture_id, count)]
    rises = [i for i in range(1, count) if levels[i] and not levels[i - 1]]
    assert len(rises) >= 5, "Expected 10 kHz PWM from harness-signals on GP5"
    measured = actual * (len(rises) - 1) / (rises[-1] - rises[0])
    assert abs(measured - 10_000) < 250, measured
    if edge:
        assert levels[0] == (edge == 1), "The first sample must match the trigger edge"
    return {
        "requested_hz": rate,
        "actual_hz": actual,
        "edge": edge,
        "pwm_hz": measured,
        "samples": count,
        "flags": flags,
        "elapsed_us": elapsed,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    probe = Probe.open(args.serial)
    try:
        results = []
        for rate in SAMPLE_RATES:
            for edge in (0, 1, 2):
                result = check_pwm(probe, rate, edge)
                results.append(result)
                print(result, flush=True)

        # CS remains high when the host sends no SPI transfer. Its rising-edge
        # trigger must time out and leave the next capture usable.
        capture_id = start(probe, 1_000_000, 1, pin=21, timeout_ms=100)
        _, _, flags, count = poll(probe, capture_id)
        assert flags & TIMEOUT and count == 0, f"Missing timeout: {flags:#x}, {count}"
        check_pwm(probe, 2_000_000, 1)

        capture_id = start(probe, 1_000_000, 1, pin=21, timeout_ms=2000)
        Probe._ok(probe.command(Command.CAPTURE_CANCEL), "cancel capture")
        _, _, flags, _ = poll(probe, capture_id)
        assert flags & CANCELLED, f"Missing cancellation: {flags:#x}"
        check_pwm(probe, 2_000_000, 2)

        report = {
            "presets": results,
            "timeout_recovery": True,
            "cancel_recovery": True,
        }
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print("PASS: 21 preset/edge combinations, trigger timeout and cancellation recovery")
    finally:
        probe.close()


if __name__ == "__main__":
    main()
