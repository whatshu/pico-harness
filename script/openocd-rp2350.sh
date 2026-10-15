#!/usr/bin/env bash
#
# openocd-rp2350.sh - start OpenOCD against the dualcore carrier to reach the
# RP2350 Work core over CMSIS-DAP.
#
# Usage:
#   script/openocd-rp2350.sh [openocd args...]
#
# Environment:
#   PDBG_PROBE_SERIAL     probe serial, used when several probes are attached
#   PDBG_ADAPTER_KHZ      adapter speed in kHz (default: 1000)
#   PDBG_OPENOCD          OpenOCD binary (default: openocd on PATH)
#   PDBG_OPENOCD_SCRIPTS  extra script search directory, passed before other args
#
# Exit status:
#   1  OpenOCD binary could not be found
set -euo pipefail

openocd="${PDBG_OPENOCD:-openocd}"
probe_serial="${PDBG_PROBE_SERIAL:-}"
adapter_khz="${PDBG_ADAPTER_KHZ:-1000}"
target_cfg="target/rp2350.cfg"

if ! command -v "$openocd" >/dev/null; then
  echo "OpenOCD not found: $openocd" >&2
  exit 1
fi

# Base configuration: CMSIS-DAP interface, adapter speed, target description.
args=(
  -f interface/cmsis-dap.cfg
  -c "adapter speed $adapter_khz"
  -f "$target_cfg"
)

# Pin OpenOCD to one probe when a serial is supplied.
if [[ -n "$probe_serial" ]]; then
  args+=(-c "adapter serial $probe_serial")
fi

# A custom script directory goes first so its target files resolve.
if [[ -n "${PDBG_OPENOCD_SCRIPTS:-}" ]]; then
  args=(-s "$PDBG_OPENOCD_SCRIPTS" "${args[@]}")
fi

exec "$openocd" "${args[@]}" "$@"
