# ph-bridge

Rust workspace for Pico Harness connectivity. `phd` owns USB, CDC, SWD, OpenOCD and picotool. `phdcli`, the desktop and Python/MCP adapters communicate with it through authenticated TCP sockets.

| Directory | Responsibility |
| --- | --- |
| `bridge/` | PH protobuf framing, CDC discovery and vendor Bulk transport |
| `device/` | Hardware operations, CMSIS-DAP, daemon-side programming and OpenOCD |
| `image/` | Shared UF2/ELF/BIN validation and address handling |
| `protocol/` | Versioned socket envelopes, errors and data types |
| `client/` | Rust socket client and desktop-compatible device interfaces |
| `phd/` | Daemon, device ownership, event history, uploads and background tasks |
| `phdcli/` | Human/JSON CLI, daemon lifecycle and native service setup |
| `proto/` | Authoritative PH and nanopb schemas |

Python scripts remain in [`host/`](../host/). Their device access uses IPC; generated protobuf types remain available for existing scripts. The MCP server and Claude/Codex adapters are in [`host/mcp/`](../host/mcp/). The former PyO3 extension and direct USB CLI have been removed.

## Build and start

Use a current stable Rust toolchain (Rust 1.88 or newer is required by the MCP SDK; the lockfiles may require a newer stable). Linux builds require a C compiler, pkg-config, libudev and libusb development files; macOS requires Xcode command-line tools and libusb. Windows builds use MSVC and libusb's bundled build fallback. A vendored protoc generates Rust protobuf code.

```sh
cargo build --release --manifest-path ph-bridge/Cargo.toml --bins
ph-bridge/target/release/phdcli daemon start
ph-bridge/target/release/phdcli doctor
ph-bridge/target/release/phdcli device list
ph-bridge/target/release/phdcli --serial SERIAL status
ph-bridge/target/release/phdcli --serial SERIAL power on
ph-bridge/target/release/phdcli daemon stop
```

On Windows use `.exe`. Put `phd` and `phdcli` in the same directory, or put both on PATH. `daemon start` locates its sibling daemon and writes diagnostics to the user state directory's `phd.log`. Run `phd` directly for foreground operation. Ctrl-C and Unix SIGTERM/SIGHUP follow the same cleanup path as `daemon stop`. Clients connect to the configured daemon explicitly; they do not silently start another daemon.

`phdcli --json` emits JSON results on stdout and structured errors on stderr. Exit codes are 0 success, 1 operation/configuration failure, 2 argument syntax, 3 device busy, and 4 socket transport failure. `--address`, `--token-file` and `--serial` are global options. Advanced `call METHOD JSON` exposes the versioned contract documented in [IPC.md](IPC.md).

## State and native services

The first daemon start creates a random token. Defaults:

| Platform | State directory |
| --- | --- |
| Linux | `$XDG_DATA_HOME/phd`, otherwise `~/.local/share/phd` |
| macOS | `~/Library/Application Support/org.PicoHarness.phd` |
| Windows | `%LOCALAPPDATA%\PicoHarness\phd\data` |

`PHD_STATE_DIR` overrides the directory; `PHD_TOKEN_FILE` overrides the token path. Unix credentials have mode 0600 and newly created state directories mode 0700. Windows files inherit the user's directory ACL. `PHD_TOKEN` can supply a token of at least 32 characters instead. An explicit file path is preferable for native services, whose environment can differ from an interactive shell.

```sh
phdcli service install
phdcli service status
phdcli service stop
phdcli service start
phdcli service uninstall
```

Installation is explicit. Linux uses a systemd user service enabled for user sessions; macOS uses a LaunchAgent for the logged-in user; Windows uses the Service Control Manager (`PicoHarness`) and requires an elevated terminal to install/remove/start/stop. These commands reference the installed daemon's absolute path, so install binaries in their permanent location first. Device access permissions belong to the daemon account. On Linux apply the existing USB/serial udev rules; on Windows the firmware advertises WinUSB for IF0 and IF3 while CDC uses usbser. Each vendor function publishes a `DeviceInterfaceGUIDs` property, as described in [Microsoft's WinUSB installation guide](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/automatic-installation-of-winusb). Older firmware may require its vendor interface driver to be configured or updated. Native service installation should be verified on each target OS.

## Remote use and GDB

Default binding is `127.0.0.1:44840`. TCP, including IPv6, is used locally and remotely. To listen on an explicit interface:

```sh
phd --listen 192.168.1.10:44840 --token-file /daemon/state/token
# On the client, with a local copy of the same token:
export PHD_ADDR=192.168.1.10:44840
export PHD_TOKEN_FILE=/local/path/remote-token
phdcli device list
phdcli --serial SERIAL debug --target rp2040
```

`debug` prints a local GDB endpoint and retains the daemon session until Ctrl-C. Connect GDB to that endpoint. The authenticated daemon socket tunnels GDB bytes to OpenOCD's loopback port; no separate OpenOCD port needs network exposure. The desktop uses the same tunnel for managed sessions. OpenOCD/picotool paths refer to the daemon host; GDB, symbols and uploaded input files belong to the client host.

The socket authenticates requests but does not encrypt traffic. Use a trusted network or forward the default loopback endpoint with SSH (`ssh -L 44840:127.0.0.1:44840 HOST`), retaining the same token on the client. Possession of the daemon token grants hardware control and access to daemon-side programmer commands.

## Programming and history

```sh
phdcli --serial SERIAL flash work.uf2 --target rp2040
phdcli task list
phdcli task status TASK_ID
phdcli task cancel TASK_ID
phdcli --serial SERIAL flash debug.uf2 --debug --wait
```

Inputs are uploaded in bounded chunks, so remote paths work without a shared filesystem. Background task IDs survive client disconnects and belong to one daemon instance. A daemon restart invalidates task IDs. Only `succeeded` confirms programming and verification. Cancellation preserves writes already completed.

Each board has one serialized control channel, independent event cursors and an exclusive SWD/debug/programming lease. Conflicting mutations return `busy`; no client automatically replays a timed-out mutation. Console readers retain the returned cursor and `instance`; `gap` explicitly reports history loss. Current PH firmware supports Bulk writes; Bulk readback remains unsupported. Logic acquisition uses connection-owned `logic.start/status/read/cancel` RPCs backed by the selected probe’s dedicated SUMP CDC. Measurements remain responsive while sampling. Firmware `pc1` supports finite 160 KiB captures with selected-channel packing and paged readback; see [memory limits](../doc/logic-memory-validation.md). Edge triggers require the `sump2` firmware tag; older firmware can use immediate captures.

The daemon retains a board's claimed Bulk interface across transactions and
releases it with the control handle on disconnect/update. A damaged control
stream is invalidated before any further request; a later explicit call can
reopen the selected serial, without replaying the failed operation. Pending
events and frame storage are bounded, and events collected by a transaction are
published before the board lock is released. SWD memory reads use
[DAP_TransferBlock](https://arm-software.github.io/CMSIS-DAP/latest/group__DAP__TransferBlock.html),
bounded by the negotiated packet size and MEM-AP's 1 KiB auto-increment boundary.

## Verification and schema generation

```sh
cargo test --manifest-path ph-bridge/Cargo.toml --workspace
cargo clippy --manifest-path ph-bridge/Cargo.toml --workspace --all-targets -- -D warnings
cargo build --manifest-path ph-bridge/Cargo.toml --bins
PYTHONPATH=host python3 -m unittest discover -s host/tests
```

Socket tests need permission to listen on loopback. Real-hardware checks are opt-in; see [testing](../doc/desktop-testing.md) and the [Linux hardware validation record](../doc/bridge-hardware-validation.md). CI runs native host builds/tests on Linux, macOS and Windows.

The authoritative schema is `ph-bridge/proto/ph.proto`. Firmware nanopb and Python generated files are checked in; Rust regenerates during builds. To regenerate the checked-in files from the repository root with nanopb/grpcio-tools installed:

```sh
python3 -m grpc_tools.protoc -I ph-bridge/proto \
  --plugin=protoc-gen-nanopb="$(command -v protoc-gen-nanopb)" \
  --nanopb_out=firmware/harness-probe/src ph-bridge/proto/ph.proto
python3 -m grpc_tools.protoc -I ph-bridge/proto --python_out=host/phbridge \
  ph-bridge/proto/ph.proto ph-bridge/proto/nanopb.proto
```

After Python generation, keep the sibling import in `ph_pb2.py` package-relative (`from . import nanopb_pb2 as nanopb__pb2`). Run the golden protobuf tests before updating generated sources.
