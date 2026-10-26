#!/usr/bin/env bash
# Keep OpenOCD on the daemon host; print the local tunneled GDB endpoint.
# PHD_ADDR/PHD_TOKEN_FILE select a local or remote daemon. Extra arguments are
# phdcli debug options (--openocd and --scripts refer to paths on that host).
set -euo pipefail
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cli="${PHDCLI:-$root_dir/ph-bridge/target/debug/phdcli}"
args=()
if [[ -n "${PHD_SERIAL:-${PDBG_PROBE_SERIAL:-}}" ]]; then
  args+=(--serial "${PHD_SERIAL:-${PDBG_PROBE_SERIAL}}")
fi
exec "$cli" "${args[@]}" debug --target rp2040 "$@"
