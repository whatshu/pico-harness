#!/usr/bin/env python3
"""Audit tracked and new source files without printing potential secret values."""
import argparse
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PATTERNS = {
    "private key": re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "AWS access key": re.compile(rb"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
    "GitHub token": re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})\b"),
    "Slack token": re.compile(rb"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
    "credential assignment": re.compile(rb"(?i)(?:password|api[_-]?key|secret[_-]?key|access[_-]?token)\s*[=:]\s*[\"'][^\"'\s]{12,}[\"']"),
}
LOCAL = {".aws", ".codex", ".agents", ".serial_mcp", ".env"}


def scan(paths):
    issues = []
    checked = 0
    for name in paths:
        path = ROOT / name
        if not path.is_file():
            continue
        if any(part in LOCAL for part in Path(name).parts):
            issues.append((name, 0, "local configuration tracked"))
            continue
        # Dependency fixtures include test credentials; audit project code and docs.
        if name.startswith("third_party/"):
            continue
        data = path.read_bytes()
        if b"\0" in data[:4096]:
            continue
        checked += 1
        for kind, pattern in PATTERNS.items():
            for match in pattern.finditer(data):
                if kind == "credential assignment" and (b"$(" in match.group() or b"${" in match.group()):
                    continue
                issues.append((name, data[:match.start()].count(b"\n") + 1, kind))
    return checked, issues


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history", action="store_true", help="Also scan every reachable project blob")
    args = parser.parse_args()
    paths = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT).decode().split("\0")
    checked, issues = scan(filter(None, paths))
    for path, line, kind in issues:
        print(f"{path}:{line}: {kind} (value redacted)")
    print(f"Audited {checked} project text files; {len(issues)} potential credential findings")
    if args.history:
        findings = set()
        objects = subprocess.check_output(["git", "rev-list", "--objects", "--all"], cwd=ROOT).decode().splitlines()
        for entry in objects:
            fields = entry.split(" ", 1)
            if len(fields) != 2 or fields[1].startswith("third_party/"):
                continue
            oid, name = fields
            kind = subprocess.check_output(["git", "cat-file", "-t", oid], cwd=ROOT).strip()
            if kind != b"blob":
                continue
            data = subprocess.check_output(["git", "cat-file", "blob", oid], cwd=ROOT)
            if b"\0" in data[:4096]:
                continue
            for label, pattern in PATTERNS.items():
                matches = list(pattern.finditer(data))
                if label == "credential assignment":
                    matches = [match for match in matches if b"$(" not in match.group() and b"${" not in match.group()]
                if matches:
                    findings.add((name, label))
        for name, kind in sorted(findings):
            print(f"history {name}: {kind} (value redacted)")
        print(f"History: {len(findings)} potential credential findings")
        issues.extend(findings)
    return bool(issues)


if __name__ == "__main__":
    raise SystemExit(main())
