#!/usr/bin/env python3
"""Verify hardware benchmark JSONL/CSV against received measurement events.

Run the desktop's ignored connected_measurement_rates_and_export test first.
Uses only Python's standard library and never accesses or changes hardware.
"""
from __future__ import annotations

import argparse
import csv
import json
import math
from pathlib import Path
import statistics

FIELDS = ["device_serial", "timestamp_unix_ms", "elapsed_s", "bus_voltage_v",
          "work_3v3_v", "current_ma", "power_mw", "power_on", "power_fault"]


def summarize(values):
    ordered = sorted(values)
    return {"min": ordered[0], "median": statistics.median(ordered),
            "p95": ordered[int((len(ordered) - 1) * .95)], "max": ordered[-1]}


def validate(root: Path):
    summary = json.loads((root / "summary.json").read_text())
    checks = []
    for run in summary["runs"]:
        if run["mode"] != "desktop-worker":
            continue
        hz = run["requested_hz"]
        directory = root / f"{hz}hz"
        recordings = list(directory.glob("*.jsonl"))
        assert len(recordings) == 1, "expected one unambiguous recording"
        header, *samples = [json.loads(line) for line in recordings[0].read_text().splitlines()]
        received = json.loads((directory / "received.json").read_text())
        with (directory / "export.csv").open(newline="") as source:
            reader = csv.DictReader(source)
            assert reader.fieldnames == FIELDS, reader.fieldnames
            rows = list(reader)
        assert len(rows) == len(samples) == len(received) == run["samples"]
        assert rows and header["version"] == 1 and header["serial"] == summary["serial"]
        assert samples[0]["elapsed_s"] == 0
        max_error = {key: 0. for key in FIELDS[2:7]}
        for index, (row, saved, event) in enumerate(zip(rows, samples, received)):
            assert row["device_serial"] == summary["serial"]
            assert int(row["timestamp_unix_ms"]) == saved["timestamp_unix_ms"] == event["timestamp_unix_ms"]
            expected = {"elapsed_s": event["elapsed_s"] - received[0]["elapsed_s"],
                        "bus_voltage_v": event["value"]["bus_mv"] / 1000.,
                        "current_ma": event["value"]["current_ma"],
                        "power_mw": event["value"]["power_mw"]}
            for key, value in expected.items():
                assert abs(saved[key] - value) < 1e-12, (index, key, saved[key], value)
            for key in max_error:
                value = float(row[key])
                assert math.isfinite(value)
                error = abs(value - saved[key])
                assert error <= .500001e-6, (index, key, error)
                max_error[key] = max(max_error[key], error)
            for key in ("power_on", "power_fault"):
                assert row[key] in ("true", "false")
                assert (row[key] == "true") == saved[key]
            if index:
                assert int(row["timestamp_unix_ms"]) > int(rows[index - 1]["timestamp_unix_ms"])
                assert float(row["elapsed_s"]) > float(rows[index - 1]["elapsed_s"])
            # Timestamps are host receipt times, with integer milliseconds.
            wall_delta = (saved["timestamp_unix_ms"] - samples[0]["timestamp_unix_ms"]) / 1000.
            assert abs(wall_delta - saved["elapsed_s"]) < .003
        assert not run["errors"], run["errors"]
        currents = [s["current_ma"] for s in samples]
        checks.append({"requested_hz": hz, "actual_hz": run["actual_hz"], "rows": len(rows),
                       "interval_ms": summarize([v * 1000 for v in run["intervals_s"]]),
                       "current_ma": summarize(currents), "current_std_ma": statistics.pstdev(currents),
                       "maximum_export_rounding_error": max_error})
    raw = json.loads((root / "saturation.json").read_text())
    currents = [row["value"]["current_ma"] for row in raw]
    current_codes_ua = sorted({round(value * 1000) for value in currents})
    observed_step_ua = math.gcd(*(b - a for a, b in zip(current_codes_ua, current_codes_ua[1:])))
    changes = [b for a, b in zip(raw, raw[1:]) if a["value"] != b["value"]]
    # Several I2C register reads can straddle an update; cluster close transitions.
    clusters = []
    for row in changes:
        if not clusters or row["elapsed_s"] - clusters[-1] > .01:
            clusters.append(row["elapsed_s"])
    saturation = {"calls":len(raw), "calls_per_second":len(raw)/raw[-1]["elapsed_s"],
                  "latency_ms": summarize([r["latency_ms"] for r in raw]),
                  "current_min_ma":min(currents), "current_max_ma":max(currents),
                  "current_std_ma":statistics.pstdev(currents),
                  "observed_current_step_ua":observed_step_ua,
                  "unchanged_tuple_fraction":1-len(changes)/(len(raw)-1),
                  "changed_value_cluster_interval_ms":summarize([(b-a)*1000 for a,b in zip(clusters,clusters[1:])])}
    result = {"passed": True, "serial": summary["serial"], "runs":checks, "saturation":saturation}
    (root / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    validate(parser.parse_args().run)
