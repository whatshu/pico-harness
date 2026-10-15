#!/usr/bin/env bash
#
# check-pinmap.sh - validate the firmware pin map against the board netlist.
#
# Usage:
#   script/check-pinmap.sh
#
# Thin wrapper around host/tools/check_pinmap.py. When the netlist is not
# present (pcb/dualcore-netlist.enet), the tool reports a skip instead of
# failing, so the script can run on a fresh checkout.
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

exec python3 "$root_dir/host/tools/check_pinmap.py" "$@"
