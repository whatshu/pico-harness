#!/usr/bin/env python3
"""Exercise a real probe through phd and save latency/integrity evidence.

The default workload reads control state and sends CRC-checked Debug Bulk
payloads. Optional Work tests restore the original rail state and require an
explicit fixture choice. No firmware is programmed by this tool.
"""
from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
from pathlib import Path
import statistics
import time

from phbridge.ipc import Client, PhdError


def connect(serial: str) -> Client:
    client = Client(timeout=15)
    try:
        client.call("device.open", {"serial": serial})
        return client
    except BaseException:
        client.close()
        raise


def summarize(samples: list[float], elapsed: float) -> dict:
    ordered = sorted(samples)
    return {
        "calls": len(ordered), "seconds": elapsed,
        "calls_per_second": len(ordered) / elapsed,
        "median_ms": statistics.median(ordered),
        "p95_ms": ordered[int((len(ordered) - 1) * .95)],
        "p99_ms": ordered[int((len(ordered) - 1) * .99)],
        "max_ms": ordered[-1],
    }


def queries(serial: str, iterations: int) -> list[float]:
    samples = []
    with connect(serial) as client:
        for index in range(iterations):
            method = ("board.info", "board.status", "board.measure", "console.status")[index % 4]
            started = time.perf_counter()
            result = client.call(method)
            if method == "board.info" and not result.startswith("PicoHarness/"):
                raise AssertionError(f"wrong device identity: {result!r}")
            samples.append((time.perf_counter() - started) * 1000)
    return samples


def crc32c(data: bytes) -> int:
    value = 0xffffffff
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = (value >> 1) ^ (0x82f63b78 if value & 1 else 0)
    return value ^ 0xffffffff


def bulk(serial: str, iterations: int) -> dict:
    results = {}
    with connect(serial) as client:
        # Exercise USB packet boundaries as well as the firmware ring limit.
        for size in (1, 63, 64, 65, 512, 4096):
            payload = bytes((index * 73 + 41) & 255 for index in range(size))
            expected_crc = crc32c(payload)
            params = {"route": "debug", "data": base64.b64encode(payload).decode("ascii")}
            samples = []
            started = time.perf_counter()
            for _ in range(iterations):
                before = time.perf_counter()
                result = client.call("bulk.write", params)
                if result != {"ok": True, "bytes": size, "crc32c": expected_crc}:
                    raise AssertionError(f"Bulk completion mismatch: {result!r}")
                samples.append((time.perf_counter() - before) * 1000)
            results[str(size)] = summarize(samples, time.perf_counter() - started)
    return results


def soak(serial: str, clients: int, seconds: float) -> dict:
    deadline = time.monotonic() + seconds
    payload = bytes((index * 73 + 41) & 255 for index in range(4096))
    params = {"route": "debug", "data": base64.b64encode(payload).decode("ascii")}
    expected = {"ok": True, "bytes": len(payload), "crc32c": crc32c(payload)}

    def worker(is_bulk: bool) -> list[float]:
        samples = []
        with connect(serial) as client:
            while time.monotonic() < deadline:
                started = time.perf_counter()
                result = client.call("bulk.write", params) if is_bulk else client.call("board.status")
                if is_bulk and result != expected:
                    raise AssertionError("Bulk integrity failed during soak")
                samples.append((time.perf_counter() - started) * 1000)
        return samples

    started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=clients + 1) as pool:
        readers = [pool.submit(worker, False) for _ in range(clients)]
        sender = pool.submit(worker, True)
        queries = [value for future in readers for value in future.result()]
        transfers = sender.result()
    elapsed = time.perf_counter() - started
    return {"queries": summarize(queries, elapsed), "bulk_4096": summarize(transfers, elapsed)}


def swd(serial: str) -> dict:
    with connect(serial) as owner, connect(serial) as competitor:
        owner.call("swd.open")
        try:
            try:
                competitor.call("swd.open")
            except PhdError as error:
                if error.kind != "busy":
                    raise
            else:
                raise AssertionError("competing SWD owner was accepted")
            owner.call("swd.connect_swd_core", {"clock_hz": 1_000_000, "core": 0})
            owner.call("swd.power_up_debug_port")
            result = {"packet_size": owner.call("swd.packet_size"), "dpid": owner.call("swd.read_dp_idcode")}
            result["cpuid"] = owner.call("swd.read_memory32", {"address": 0xe000ed00, "words": 1})
            timings = {}
            for words in (1, 16, 256, 1024):
                started = time.perf_counter()
                values = owner.call("swd.read_memory32", {"address": 0x10000000, "words": words})
                if len(values) != words:
                    raise AssertionError("incomplete SWD memory response")
                timings[str(words)] = (time.perf_counter() - started) * 1000
            result["flash_read_ms"] = timings
            # Compare with independent single-register transfers, including
            # reads crossing the MEM-AP's 1 KiB auto-increment boundary.
            for address, words in ((0x10000000, 16), (0x100003f0, 64), (0x10000ff0, 64)):
                block = owner.call("swd.read_memory32", {"address": address, "words": words})
                owner.call("swd.write_ap", {"address": 0, "value": 0x23000052})
                scalar = []
                for index in range(words):
                    owner.call("swd.write_ap", {"address": 4, "value": address + index * 4})
                    scalar.append(owner.call("swd.read_ap", {"address": 12}))
                if block != scalar:
                    raise AssertionError(f"block/scalar Flash mismatch at {address:#x}")
            try:
                owner.call("swd.read_memory32", {"address": 0xfffffff0, "words": 1})
            except PhdError as error:
                if error.kind != "device_error":
                    raise
            else:
                raise AssertionError("unmapped memory did not report a target fault")
            recovered = owner.call("swd.read_memory32", {"address": 0xe000ed00, "words": 1})
            if recovered != result["cpuid"]:
                raise AssertionError("SWD failed to recover after an invalid memory read")
            result["cross_boundary_scalar_comparison"] = True
            result["fault_recovery"] = True
        finally:
            # Closing the socket deliberately exercises daemon-owned cleanup.
            owner.close()
        deadline = time.monotonic() + 3
        while True:
            try:
                competitor.call("swd.open")
                break
            except PhdError as error:
                if error.kind != "busy" or time.monotonic() >= deadline:
                    raise
                time.sleep(.01)
        competitor.call("swd.disconnect")
        return result


def console_echo(serial: str) -> dict:
    with connect(serial) as writer, connect(serial) as reader:
        cursors = [client.call("device.open", {"serial": serial})["cursor"]
                   for client in (writer, reader)]
        received = [bytearray(), bytearray()]
        payload = bytes((index * 73 + 41) & 255 for index in range(4096))
        started = time.perf_counter()
        for offset in range(0, len(payload), 128):
            writer.call("console.write", {"data": base64.b64encode(payload[offset:offset + 128]).decode("ascii")})
            deadline = time.monotonic() + 3
            for index, client in enumerate((writer, reader)):
                while len(received[index]) < offset + 128:
                    if time.monotonic() >= deadline:
                        raise AssertionError("Console echo timed out; verify byte-echo fixture firmware")
                    batch = client.call("events.read", {"cursor": cursors[index], "limit": 256, "wait_ms": 100})
                    cursors[index] = batch["cursor"]
                    if batch["gap"] or batch["instance"] != client.instance:
                        raise AssertionError("Console history was lost")
                    for event in batch["events"]:
                        if "disconnected" in event:
                            raise AssertionError(f"Console device disconnected: {event!r}")
                        if "console" in event:
                            received[index].extend(base64.b64decode(event["console"], validate=True))
        if any(data != payload for data in received):
            raise AssertionError("Console bytes differ, or subscribers consumed each other's data")
        return {"bytes_per_subscriber": len(payload), "subscribers": 2,
                "seconds": time.perf_counter() - started}


def console_terminal(serial: str, iterations: int) -> dict:
    with connect(serial) as writer, connect(serial) as reader, connect(serial) as slow:
        cursors = [client.call("device.open", {"serial": serial})["cursor"]
                   for client in (writer, reader, slow)]
        total_bytes = 0
        started = time.perf_counter()
        for iteration in range(iterations):
            command = b"help\n" if iteration % 2 == 0 else b"info\n"
            writer.call("console.write", {"data": base64.b64encode(command).decode("ascii")})
            outputs = []
            for index, client in enumerate((writer, reader)):
                output = bytearray()
                deadline = time.monotonic() + 3
                while not output.endswith(b"harness> "):
                    if time.monotonic() >= deadline:
                        raise AssertionError("harness-terminal prompt timed out")
                    batch = client.call("events.read", {"cursor": cursors[index], "limit": 256, "wait_ms": 100})
                    cursors[index] = batch["cursor"]
                    if batch["gap"] or batch["instance"] != client.instance:
                        raise AssertionError("active Console subscriber lost history")
                    for event in batch["events"]:
                        if "disconnected" in event:
                            raise AssertionError(f"Console disconnected: {event!r}")
                        if "console" in event:
                            output.extend(base64.b64decode(event["console"], validate=True))
                outputs.append(bytes(output))
            expected = b"help                  this help" if command == b"help\n" else b"firmware: harness-terminal"
            if outputs[0] != outputs[1] or expected not in outputs[0]:
                raise AssertionError("terminal reply was incomplete or differed between subscribers")
            total_bytes += len(outputs[0])
        history = slow.call("events.read", {"cursor": cursors[2], "limit": 256})
        return {"commands": iterations, "bytes_per_subscriber": total_bytes,
                "subscribers": 2, "seconds": time.perf_counter() - started,
                "slow_subscriber_gap": history["gap"]}


def run(args, report: dict) -> None:
    with connect(args.serial) as client:
        report["daemon"] = client.call("daemon.health")
        report["firmware"] = client.call("board.info")
        report["initial_status"] = client.call("board.status")
        report["terminal"] = client.call("console.status")
    started = time.perf_counter()
    report["sequential"] = summarize(queries(args.serial, args.iterations), time.perf_counter() - started)
    with ThreadPoolExecutor(max_workers=args.clients) as pool:
        started = time.perf_counter()
        samples = [value for batch in pool.map(lambda _: queries(args.serial, args.iterations), range(args.clients))
                   for value in batch]
        report["concurrent"] = summarize(samples, time.perf_counter() - started)
        started = time.perf_counter()
        futures = [pool.submit(queries, args.serial, args.iterations) for _ in range(args.clients)]
        report["bulk_under_query_load"] = bulk(args.serial, args.bulk_iterations)
        samples = [value for future in futures for value in future.result()]
        report["mixed_queries"] = summarize(samples, time.perf_counter() - started)
    report["bulk"] = bulk(args.serial, args.bulk_iterations)
    started = time.perf_counter()
    for _ in range(args.churn):
        with connect(args.serial) as client:
            client.call("board.info")
    report["connection_churn"] = {"connections": args.churn, "seconds": time.perf_counter() - started}
    if args.soak_seconds:
        report["soak"] = soak(args.serial, args.clients, args.soak_seconds)
    if args.swd_rp2040 or args.console_echo or args.console_terminal:
        with connect(args.serial) as client:
            initial = client.call("board.status")["power_on"]
            try:
                client.call("board.power", {"enabled": True})
                time.sleep(.5)
                status = client.call("board.status")
                if not status["power_on"] or not status["power_good"] or status["power_fault"]:
                    raise AssertionError(f"Work power did not become ready: {status!r}")
                if args.swd_rp2040:
                    report["swd_rp2040"] = swd(args.serial)
                if args.console_echo:
                    report["console_echo"] = console_echo(args.serial)
                if args.console_terminal:
                    report["console_terminal"] = console_terminal(args.serial, args.console_iterations)
            finally:
                client.call("board.power", {"enabled": initial})
    report["ok"] = True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--iterations", default=400, type=int)
    parser.add_argument("--clients", default=8, type=int)
    parser.add_argument("--bulk-iterations", default=20, type=int)
    parser.add_argument("--churn", default=100, type=int)
    parser.add_argument("--soak-seconds", default=0, type=float, help="duration of sustained concurrent status/4 KiB Bulk traffic")
    parser.add_argument("--swd-rp2040", action="store_true", help="power Work temporarily and read RP2040 memory")
    parser.add_argument("--console-echo", action="store_true", help="requires byte-echo firmware already on Work; temporarily enables power")
    parser.add_argument("--console-terminal", action="store_true", help="requires harness-terminal already on Work; checks two subscribers with help/info commands")
    parser.add_argument("--console-iterations", default=20, type=int)
    args = parser.parse_args()
    if not 1 <= args.clients <= 32 or min(args.iterations, args.bulk_iterations, args.churn, args.console_iterations) < 1:
        parser.error("clients must be 1..32 and iteration counts must be positive")
    if args.soak_seconds and not 1 <= args.soak_seconds <= 3600:
        parser.error("soak duration must be 1..3600 seconds")
    report = {"serial": args.serial, "time": datetime.now(timezone.utc).isoformat(), "ok": False}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        run(args, report)
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
