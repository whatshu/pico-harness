#!/usr/bin/env bash
# Upload a Debug UF2 and let phd perform the update on its selected USB port.
# PICOTOOL, when set, names the programmer on the daemon host.
set -euo pipefail
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
uf2="${1:?usage: script/enter-debug-update.sh path/to/debug.uf2}"
args=()
if [[ -n "${PHD_SERIAL:-${PDBG_SERIAL:-}}" ]]; then
  args+=(-p "${PHD_SERIAL:-${PDBG_SERIAL}}")
fi
exec env PYTHONPATH="$root_dir/host${PYTHONPATH:+:$PYTHONPATH}" \
  python3 -m phbridge "${args[@]}" flash "$uf2"
