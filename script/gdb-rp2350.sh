#!/usr/bin/env bash
#
# gdb-rp2350.sh - start GDB for the RP2350 Debug Core.
#
# Usage:
#   script/gdb-rp2350.sh [gdb args...]
#
# Environment:
#   PDBG_GDB  explicit GDB binary; otherwise gdb-multiarch or arm-none-eabi-gdb
#             is looked up on PATH.
#
# Exit status:
#   1  no usable GDB binary could be found
set -euo pipefail

# Resolve the GDB binary, honouring an explicit override first.
gdb="${PDBG_GDB:-}"
if [[ -z "$gdb" ]]; then
  gdb="$(command -v gdb-multiarch || command -v arm-none-eabi-gdb || true)"
fi

if [[ -z "$gdb" ]]; then
  echo "gdb-multiarch or arm-none-eabi-gdb not found" >&2
  exit 1
fi

exec "$gdb" "$@"
