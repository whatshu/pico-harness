#!/usr/bin/env bash
#
# pdbgctl.sh - run the host PDBG command-line tool.
#
# Usage:
#   script/pdbgctl.sh <subcommand> [args...]
#
# Prepends the repository's host/ package to PYTHONPATH and execs the pdbg
# CLI, so the script can be used directly as a drop-in entry point.
set -euo pipefail

# Resolve the repository root from this script's location.
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

exec env \
  PYTHONPATH="$root_dir/host${PYTHONPATH:+:$PYTHONPATH}" \
  python3 -m pdbg.cli "$@"
