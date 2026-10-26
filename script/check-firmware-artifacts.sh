#!/usr/bin/env bash
#
# check-firmware-artifacts.sh - validate freshly built Debug Core artifacts.
#
# Usage:
#   script/check-firmware-artifacts.sh
#
# Thin wrapper that keeps the checker invocable without typing out the full
# host/ tool path.
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

exec python3 "$root_dir/host/tools/check_firmware_artifacts.py" "$@"
