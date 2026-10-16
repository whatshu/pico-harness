#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
build_dir="$root_dir/BUILD"
release_dir="$root_dir/RELEASE"
version="$(tr -d '[:space:]' < "$root_dir/VERSION")"
command_name="${1:-build}"
shift || true

usage() { echo "Usage: ./build.sh [build|build-debug|flash|monitor|test [--hardware|--offline]|clean|ci]"; }

require_command() {
  command -v "$1" >/dev/null 2>&1 || { echo "missing required command: $1" >&2; return 1; }
}

check_dependencies() {
  require_command cmake
  require_command ninja
  require_command python3
  require_command arm-none-eabi-gcc
  test -n "${PICO_SDK_PATH:-}" && test -f "$PICO_SDK_PATH/pico_sdk_init.cmake" || {
    echo "PICO_SDK_PATH must point to the installed Pico SDK" >&2; return 1;
  }
}

configure_cmake() {
  local source_dir="$1" output_dir="$2"
  shift 2
  # Build directories may have been created by the old container layout. A
  # clean configure also removes nested pioasm caches with stale absolute paths.
  if [[ -d "$output_dir" ]]; then cmake -E remove_directory "$output_dir"; fi
  cmake -S "$source_dir" -B "$output_dir" "$@"
}

build_debug() {
  check_dependencies || return
  test -d "$root_dir/third_party/FreeRTOS-Kernel/portable/ThirdParty" || {
    echo "missing third_party/FreeRTOS-Kernel" >&2; return 1;
  }
  local output_dir="$build_dir/harness-probe-pico"
  local name=pico_dualcore_debug_pico
  local artifacts="$release_dir/artifacts"
  mkdir -p "$artifacts"
  configure_cmake "$root_dir/firmware/harness-probe" "$output_dir" \
    -G Ninja -DPICO_BOARD=pico_dualcore_pico -DCMAKE_BUILD_TYPE=RelWithDebInfo || return
  cmake --build "$output_dir" || return
  cp "$output_dir/$name.uf2" "$output_dir/$name.elf" "$artifacts/" || return
  rm -f "$artifacts/pico_dualcore_debug_pico2.uf2" \
    "$artifacts/pico_dualcore_debug_pico2.elf"
  python3 "$root_dir/host/tools/check_firmware_artifacts.py" || return
  (cd "$artifacts" && sha256sum "$name.uf2" "$name.elf" > SHA256SUMS)
  cat > "$artifacts/build-info.yaml" <<EOF2
boards: [pico_dualcore_pico]
hardware: pcb/pico-dualcore-devboard-v1.2
version: "$version"
source: "$(git -C "$root_dir" describe --always --tags --dirty 2>/dev/null || echo unknown)"
built_at: "$(date -Iseconds)"
command: ./build.sh build
uf2:
  pico: pico_dualcore_debug_pico.uf2
EOF2
}

build_work_selftest() {
  check_dependencies || return
  local output_dir="$build_dir/work-selftest"
  configure_cmake "$root_dir/firmware/test/work-selftest" "$output_dir" \
    -G Ninja -DPICO_BOARD=pico -DCMAKE_BUILD_TYPE=RelWithDebInfo || return
  cmake --build "$output_dir"
}

run_tests() {
  PYTHONPATH="$root_dir/host" python3 -m unittest discover -s "$root_dir/host/tests" -p 'test_*.py' || return
  cargo test --manifest-path "$root_dir/host/ph-bridge/Cargo.toml" --lib || return
  bash -n "$root_dir/build.sh" || return
  for wrapper in "$root_dir"/script/*.sh; do bash -n "$wrapper" || return; done
  python3 "$root_dir/host/tools/check_pinmap.py"
}

run_test_suite() {
  local hardware_mode="${1:-auto}" test_dir="$release_dir/test"
  run_tests || return
  build_work_selftest || return
  mkdir -p "$test_dir"
  cp "$build_dir/work-selftest/work_selftest.uf2" \
    "$build_dir/work-selftest/work_selftest.elf" \
    "$build_dir/work-selftest/work_selftest_ram.elf" "$test_dir/" || return
  (cd "$test_dir" && sha256sum work_selftest.uf2 work_selftest.elf \
    work_selftest_ram.elf > SHA256SUMS) || return
  echo "Work fixtures built; hardware work self-test superseded by the PH"
  echo "device integration suite (host/tests/test_ph_device.py)."
}


flash_debug() {
  require_command python3 || return
  require_command "${PICOTOOL:-picotool}" || return
  local uf2
  uf2="$(ls "$build_dir"/pico_dualcore_debug_*.uf2 2>/dev/null | head -1)"
  [[ -n "$uf2" ]] || uf2="$(ls "$release_dir"/artifacts/pico_dualcore_debug_*.uf2 2>/dev/null | head -1)"
  [[ -n "$uf2" ]] || { echo "no debug UF2 built ($build_dir/pico_dualcore_debug_*.uf2)"; return 1; }
  PICOTOOL="${PICOTOOL:-picotool}" PYTHONPATH="$root_dir/host" python3 -m phbridge flash "$uf2"
}

monitor_debug() {
  echo "monitor: retired with the PDBG wire protocol; use the WORK console"
  echo "over the PH Terminal instead: ph console read"
}

run_ci() {
  local stamp started finished commit log info status failed_stage
  stamp="$(date +%Y%m%d-%H%M%S)"; started="$(date -Iseconds)"
  commit="$(git -C "$root_dir" rev-parse HEAD 2>/dev/null || echo none)"
  log="$release_dir/logs/build-$stamp.log"; info="$release_dir/build-info-$stamp.yaml"
  status=success; failed_stage=none
  mkdir -p "$release_dir/logs" "$release_dir/artifacts"
  {
    for stage in build test artifact-scenario-test; do
      echo "==> stage: $stage"
      case "$stage" in
        build) build_debug || { status=failed; failed_stage=$stage; break; } ;;
        test) run_test_suite offline || { status=failed; failed_stage=$stage; break; } ;;
        artifact-scenario-test)
          test -s "$release_dir/artifacts/pico_dualcore_debug_pico.uf2" || { status=failed; failed_stage=$stage; break; }
          test -s "$release_dir/test/work_selftest.uf2" || { status=failed; failed_stage=$stage; break; }
          ;;
      esac
    done
  } >"$log" 2>&1
  finished="$(date -Iseconds)"
  cat > "$info" <<EOF2
build:
  status: "$status"
  version: "$version"
  commit_id: "$commit"
  started_at: "$started"
  finished_at: "$finished"
  failed_stage: "$failed_stage"
  logs:
    - path: "RELEASE/logs/$(basename "$log")"
  artifacts:
    - path: "RELEASE/artifacts"
    - path: "RELEASE/test"
EOF2
  if [[ "$status" != success ]]; then echo "CI failed at $failed_stage; see $log" >&2; return 1; fi
  echo "CI succeeded; see $info"
}

case "$command_name" in
  build) [[ $# -eq 0 ]] || { usage >&2; exit 2; }; build_debug ;;
  build-debug) build_debug ;;
  flash) [[ $# -eq 0 ]] || { usage >&2; exit 2; }; flash_debug ;;
  monitor) monitor_debug "$@" ;;
  test)
    case "${1:-}" in
      "") run_test_suite ;;
      --hardware) [[ $# -eq 1 ]] || { usage >&2; exit 2; }; run_test_suite required ;;
      --offline) [[ $# -eq 1 ]] || { usage >&2; exit 2; }; run_test_suite offline ;;
      *) usage >&2; exit 2 ;;
    esac ;;
  clean) rm -rf "$build_dir" "$release_dir"; mkdir -p "$release_dir"; touch "$release_dir/.gitkeep" ;;
  ci) run_ci ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
