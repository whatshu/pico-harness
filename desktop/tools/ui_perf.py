#!/usr/bin/env python3
"""Measure the release desktop's native UI on Linux, without extra Python packages."""
import argparse
import csv
import hashlib
import json
import math
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCENARIOS = {
    "idle": "No device; USB discovery disabled",
    "live": "20 measurements/s; 1,200 preloaded samples",
    "live-record-paused": "20 measurements/s recorded to JSONL; 7,200 frozen samples",
    "console": "20 messages/s, 64 B/message; 200 preloaded rows",
    "console-flood": "1 MiB/s, 1 KiB/message; 5,000 rows and full 16 MiB receive buffer",
    "console-hex": "512 KiB/s, 4 KiB/message; 5,000 rows in hex; full receive buffer",
    "trace-bulk": "1 MiB/s SPI trace, 2 KiB/chunk; full 2 MiB trace; trace window open",
    "history-limit": "Load and display a 200,000-sample JSONL recording",
    "logic": "32,768 samples, 16 channels and four decoder instances",
    "logic-dense": "32,768 samples, 16 dense channels and eight PWM decoder instances",
    "swd-registers": "Running target with cached registers for both cores",
    "swd-memory": "Halted target with a 256-byte memory inspection",
    "gdb-source": "Disconnected GDB page with a cached 2,000-line source file",
    "firmware": "Idle firmware upload page; no programming",
    "connected-live": "Actual selected USB probe, Live measurements; no programming",
}
MIB = 1024 * 1024


def proc_cpu_ticks(stat):
    # comm can contain spaces and parentheses; all numeric fields follow its final ')'.
    fields = stat.rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def read_process(pid):
    proc = Path("/proc") / str(pid)
    before = time.monotonic()
    ticks = proc_cpu_ticks((proc / "stat").read_text())
    fields = (proc / "statm").read_text().split()
    rss = int(fields[1]) * os.sysconf("SC_PAGE_SIZE") / MIB
    pss = None
    try:
        for line in (proc / "smaps_rollup").read_text().splitlines():
            if line.startswith("Pss:"):
                pss = int(line.split()[1]) / 1024
                break
    except (PermissionError, FileNotFoundError):
        pass
    return {"time": (before + time.monotonic()) / 2,
            "cpu_s": ticks / os.sysconf("SC_CLK_TCK"), "rss_mib": rss, "pss_mib": pss}


def slope(samples):
    if len(samples) < 2:
        return 0.0
    center_t = statistics.mean(s["time"] for s in samples)
    center_y = statistics.mean(s["rss_mib"] for s in samples)
    denominator = sum((s["time"] - center_t) ** 2 for s in samples)
    return sum((s["time"] - center_t) * (s["rss_mib"] - center_y) for s in samples) / denominator if denominator else 0.0


def summarize(samples, start, duration):
    steady = [s for s in samples if start <= s["time"] <= start + duration]
    intervals = [(b["time"] - a["time"], b["cpu_s"] - a["cpu_s"])
                 for a, b in zip(steady, steady[1:]) if b["time"] > a["time"]]
    if len(intervals) < 3:
        raise RuntimeError("Insufficient process samples after warmup")
    cpu = [100 * work / elapsed for elapsed, work in intervals]
    pss = [s["pss_mib"] for s in steady if s["pss_mib"] is not None]
    return {
        "samples": len(steady), "sampled_duration_s": sum(dt for dt, _ in intervals),
        "cpu_mean_pct_one_core": 100 * sum(work for _, work in intervals) / sum(dt for dt, _ in intervals),
        "cpu_peak_pct_one_core": max(cpu),
        "rss_mean_mib": statistics.mean(s["rss_mib"] for s in steady),
        "rss_peak_mib": max(s["rss_mib"] for s in steady),
        "rss_startup_peak_mib": max(s["rss_mib"] for s in samples),
        "pss_mean_mib": statistics.mean(pss) if pss else None,
        "rss_growth_mib": steady[-1]["rss_mib"] - steady[0]["rss_mib"],
        "rss_slope_mib_per_s": slope(steady),
    }


def validate_fixture(scenario, ui):
    errors = []
    if ui["frames"] < 3:
        errors.append("Fewer than three measured frames")
    buffers = ui.get("buffers") or {}
    caps = {"console_rows": 5000, "received_bytes": 16 * MIB, "trace_rows": 4000,
            "trace_bytes": 2 * MIB, "live_samples": 7200, "history_samples": 200000,
            "logic_samples": 32768}
    for key, limit in caps.items():
        if buffers.get(key, 0) > limit:
            errors.append(f"{key} exceeded its bound")
    if scenario == "connected-live" and (not buffers.get("live_ready") or buffers.get("live_samples", 0) < 10):
        errors.append("Selected probe did not provide Live measurements")
    if scenario == "history-limit" and buffers.get("history_samples") != 200000:
        errors.append("Full history fixture was not displayed")
    if scenario == "live-record-paused" and not (buffers.get("recording") and buffers.get("paused") and buffers.get("history_samples", 0) > 0):
        errors.append("Paused recording fixture did not record")
    if scenario in {"console-flood", "console-hex"} and (buffers.get("console_rows") != 5000 or buffers.get("received_bytes") != 16 * MIB):
        errors.append("Console fixture did not fill both bounded buffers")
    if scenario == "trace-bulk" and buffers.get("trace_bytes") != 2 * MIB:
        errors.append("Trace fixture did not fill the bounded buffer")
    if scenario.startswith("logic") and (buffers.get("logic_samples") != 32768 or buffers.get("decoded_events", 0) == 0):
        errors.append("Logic fixture did not render a capture and decoded events")
    return errors


def run_scenario(args, scenario, repeat):
    directory = args.output / f"{scenario}-{repeat}"
    directory.mkdir(parents=True, exist_ok=False)
    output = directory / "ui.json"
    command = [str(args.binary), "--perf-scenario", scenario, "--perf-output", str(output),
               "--perf-warmup", str(args.warmup), "--perf-duration", str(args.duration), "--language", "en"]
    if scenario == "connected-live":
        command += ["--perf-probe", args.probe_serial]
    env = dict(os.environ, PDBG_RECORDINGS=str(directory / "recordings"))
    samples = []
    ready = None
    ready_clock = None
    started = time.monotonic()
    with (directory / "process.log").open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=log)
        try:
            while process.poll() is None:
                try:
                    samples.append(read_process(process.pid))
                except (FileNotFoundError, ProcessLookupError):
                    pass
                if ready is None and output.with_suffix(".ready.json").exists():
                    ready = json.loads(output.with_suffix(".ready.json").read_text())
                    # Calibrate the native readiness timestamp onto the observer's monotonic clock.
                    ready_clock = time.monotonic() - (time.time() - ready["started_unix_ms"] / 1000)
                limit = (ready_clock + args.warmup + args.duration + 45) if ready else (started + 60)
                if time.monotonic() > limit:
                    raise RuntimeError("Native scenario timed out; see process.log")
                time.sleep(args.interval)
            if process.returncode != 0:
                raise RuntimeError(f"Desktop exited {process.returncode}; see process.log")
            if ready is None or not output.exists():
                raise RuntimeError("Desktop did not emit its performance report; build with --features ui-perf")
            ui = json.loads(output.read_text())
            metrics = summarize(samples, ready_clock + args.warmup, ui["observed_duration_s"])
            failures = validate_fixture(scenario, ui)
            for key, budget in (("cpu_mean_pct_one_core", args.max_cpu), ("rss_peak_mib", args.max_rss)):
                if budget is not None and metrics[key] > budget:
                    failures.append(f"{key} exceeded budget {budget}")
            if args.max_update is not None and ui["update_ms"]["p95"] > args.max_update:
                failures.append(f"Update p95 exceeded {args.max_update} ms")
            if args.max_render is not None and (not ui["render_cpu_ms"] or ui["render_cpu_ms"]["p95"] > args.max_render):
                failures.append(f"Render CPU p95 exceeded {args.max_render} ms")
            with (directory / "process.csv").open("w", newline="") as file:
                writer = csv.DictWriter(file, fieldnames=["elapsed_s", "cpu_s", "rss_mib", "pss_mib"])
                writer.writeheader()
                for sample in samples:
                    writer.writerow(dict(elapsed_s=sample["time"] - ready_clock,
                                         **{key: sample[key] for key in ("cpu_s", "rss_mib", "pss_mib")}))
            return {"scenario": scenario, "repeat": repeat, "workload": SCENARIOS[scenario],
                    "metrics": metrics, "ui": ui, "failures": failures}
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def markdown(report):
    lines = ["# PicoHarness native UI performance", "",
             "CPU: 100% = one logical CPU, summed across application threads. Memory in MiB.",
             "Warmup is excluded. Frame interval includes the application's repaint wait; update time does not.",
             "GPU VRAM and the compositor are not included; PSS apportions shared resident mappings.", "",
             "| Scenario | CPU mean / peak % | RSS mean / peak MiB | PSS mean MiB | RSS slope MiB/s | Update p95 ms | Render CPU p95 ms | Frames/s | Status |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |"]
    for result in report["results"]:
        if "metrics" not in result:
            lines.append(f"| {result['scenario']} | — | — | — | — | — | — | — | {result['error']} |")
            continue
        m, ui = result["metrics"], result["ui"]
        pss = f"{m['pss_mean_mib']:.1f}" if m["pss_mean_mib"] is not None else "unavailable"
        render = f"{ui['render_cpu_ms']['p95']:.2f}" if ui["render_cpu_ms"] else "unavailable"
        lines.append(f"| {result['scenario']} #{result['repeat']} | {m['cpu_mean_pct_one_core']:.1f} / {m['cpu_peak_pct_one_core']:.1f} | "
                     f"{m['rss_mean_mib']:.1f} / {m['rss_peak_mib']:.1f} | {pss} | {m['rss_slope_mib_per_s']:.2f} | "
                     f"{ui['update_ms']['p95']:.2f} | {render} | {ui['frames_per_s']:.1f} | {'FAIL' if result['failures'] else 'PASS'} |")
    lines += ["", "## Environment", "", "```json", json.dumps(report["environment"], indent=2), "```", "", "## Workloads", ""]
    for scenario in dict.fromkeys(result["scenario"] for result in report["results"]):
        lines.append(f"- **{scenario}**: {SCENARIOS[scenario]}")
    lines += ["", "RSS growth over a short run can include allocator/font/driver caches and does not establish a leak.",
              "Synthetic cases include event generation/ingestion cost but exclude physical USB throughput and latency.",
              "Logic cases hold one decoded capture; they measure display cost, not repeated acquisition/decoding.",
              "Use repeated runs on the same machine and renderer when enforcing budgets."]
    return "\n".join(lines) + "\n"


def finite_positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("Expected a finite positive number")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "desktop/target/release/picoharness-desktop")
    parser.add_argument("--output", type=Path, required=True, help="New directory for raw samples, reports and fixture files")
    parser.add_argument("--scenario", action="append", choices=SCENARIOS, help="Repeat to select cases; defaults to all synthetic cases")
    parser.add_argument("--probe-serial", help="Explicit probe for --scenario connected-live; never saved to reports")
    parser.add_argument("--warmup", type=finite_positive, default=3)
    parser.add_argument("--duration", type=finite_positive, default=10)
    parser.add_argument("--interval", type=finite_positive, default=0.25)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--max-cpu", type=finite_positive, help="Fail above mean CPU %% of one core")
    parser.add_argument("--max-rss", type=finite_positive, help="Fail above steady RSS peak in MiB")
    parser.add_argument("--max-update", type=finite_positive, help="Fail above UI update p95 in ms")
    parser.add_argument("--max-render", type=finite_positive, help="Fail above eframe CPU-side frame p95 in ms")
    args = parser.parse_args()
    if sys.platform != "linux" or not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        parser.error("Requires Linux /proc and a usable native X11/Wayland display")
    if args.repeat < 1 or args.warmup > 600 or args.duration > 600 or args.interval > min(args.duration / 4, 1):
        parser.error("Invalid repeat count, duration, warmup or sampling interval")
    args.scenario = args.scenario or [name for name in SCENARIOS if name != "connected-live"]
    if "connected-live" in args.scenario and not args.probe_serial:
        parser.error("connected-live requires --probe-serial")
    args.binary = args.binary.resolve()
    if not args.binary.is_file():
        parser.error("Build the release binary with --features ui-perf first")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), platform.processor())
    report = {
        "schema_version": 1,
        "environment": {"os": platform.platform(), "cpu": cpu, "logical_cpus": os.cpu_count(),
                        "viewport_logical_points": [1360, 940], "build": "release, ui-perf, Glow",
                        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                        "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                        "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip()),
                        "warmup_s": args.warmup, "duration_s": args.duration, "sample_interval_s": args.interval},
        "results": [],
    }
    for scenario in args.scenario:
        for repeat in range(1, args.repeat + 1):
            print(f"Running {scenario} #{repeat}...", flush=True)
            try:
                result = run_scenario(args, scenario, repeat)
                print(f"  CPU {result['metrics']['cpu_mean_pct_one_core']:.1f}%; RSS peak {result['metrics']['rss_peak_mib']:.1f} MiB; update p95 {result['ui']['update_ms']['p95']:.2f} ms", flush=True)
            except (OSError, RuntimeError, ValueError, KeyError) as error:
                result = {"scenario": scenario, "repeat": repeat, "error": str(error)}
                print(f"  FAILED: {error}", flush=True)
            report["results"].append(result)
            (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
            (args.output / "summary.md").write_text(markdown(report))
    print(f"Results: {args.output / 'summary.md'}")
    return int(any(result.get("error") or result.get("failures") for result in report["results"]))


if __name__ == "__main__":
    sys.exit(main())
