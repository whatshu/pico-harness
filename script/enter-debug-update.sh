#!/usr/bin/env bash
#
# enter-debug-update.sh - enter ROM BOOTSEL through the live PDBG Link, then
# load a family-specific UF2 into the Debug Core with picotool.
#
# Usage:
#   script/enter-debug-update.sh path/to/pico_dualcore_debug_pico.uf2
#
# Environment:
#   PDBG_SERIAL    select a specific probe when several are attached
#   PICOTOOL       override the picotool binary (default: picotool on PATH)
#
# Exit status:
#   1  picotool is missing
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# Required first argument: the UF2 image to load.
uf2="${1:?usage: script/enter-debug-update.sh path/to/pico_dualcore_debug_pico.uf2}"

# Optional probe selection.
serial="${PDBG_SERIAL:-}"

# Locate picotool; an explicit PICOTOOL wins over PATH lookup.
picotool="${PICOTOOL:-$(command -v picotool || true)}"
if [[ -z "$picotool" ]]; then
  echo "picotool not found; set PICOTOOL or install it in PATH" >&2
  exit 1
fi

# Assemble the update command; constrain to the probe when one is given.
args=()
if [[ -n "$serial" ]]; then
  args+=(--serial "$serial")
fi

PYTHONPATH="$root_dir/host" python3 -m pdbg.cli "${args[@]}" \
  update "$uf2" --picotool "$picotool"
