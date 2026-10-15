# VS Code setup (dualcore local development)

The workspace root must be `firmware/harness-probe` (the directory holding this
`.vscode`). The project supports exactly the two Debug Core targets of
`pcb/pico-dualcore-devboard-v1.2`: `pico_dualcore` (Pico 2) and `pico_dualcore_pico` (RP2040).

## Division of labour

| Task | Tool | Where |
|------|------|------|
| IntelliSense / navigation | system `clangd` | reads `<repo>/BUILD/analysis/<board>/compile_commands.json` |
| Real builds | local `./build.sh` | outputs to `<repo>/BUILD/...` and `RELEASE/artifacts` |
| Flashing | local `picotool` | Flash tasks |
| Debugging | local `openocd` + `gdb-multiarch` | `script/openocd-rp2350.sh` |

## First use

### 1. Check the local toolchain

```bash
echo "$PICO_SDK_PATH"
arm-none-eabi-gcc --version
picotool version
gdb-multiarch --version
openocd --version
```

`PICO_SDK_PATH` must point to an installed Pico SDK that contains
`pico_sdk_init.cmake`.

### 2. Generate the compile database

In VS Code's "Run Task" pick:

- `Analysis: configure dualcore Pico 2`
- `Analysis: configure dualcore Pico`

Each run configures `<repo>/BUILD/analysis/<board>` with the local SDK and
toolchain, builds `harness-probe` to generate the PIO headers and `version.h`,
then symlinks `${workspaceFolder}/build` to that directory. When switching
boards, run `Analysis: switch active board` and then `clangd: Restart language
server`.

### 3. Build, flash and debug

- Build: `Build: dualcore Debug Core images`, or `Build: full firmware suite`
  (includes the Work self-test)
- Checks: `Test: host and dualcore checks`
- Flash: put the target into BootSel, then run the matching `Flash: dualcore
  ...` or `Flash: Work self-test`
- Debug: press F5 and pick `Debug dualcore — Pico 2 core` or `Debug dualcore —
  Pico core (RP2040)`

The OpenOCD tasks call `script/openocd-rp2350.sh` and the `openocd` on
`PATH`. To select a specific probe, set `PDBG_PROBE_SERIAL`; the adapter speed
can be overridden with `PDBG_ADAPTER_KHZ`.

## Troubleshooting

- **clangd cannot find headers / wrong macros**: run the matching Analysis
  configure task, make sure the `firmware/harness-probe/build` symlink exists,
  then restart clangd.
- **`version.h` or PIO headers missing**: re-run the matching Analysis
  configure task.
- **Debug cannot connect**: make sure local `openocd` runs, the probe is
  attached, and check `PDBG_PROBE_SERIAL`.

## Files involved

- `.vscode/extensions.json` — recommends clangd + cortex-debug
- `.vscode/settings.json` — clangd options, build directory, local SDK
- `.vscode/tasks.json` — analysis config, local builds, tests, flashing,
  OpenOCD start/stop
- `.vscode/launch.json` — the two dualcore cortex-debug targets