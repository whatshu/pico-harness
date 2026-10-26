# phd socket contract, version 1

The same TCP protocol serves local and remote clients. Each connection performs `hello`, then sends one outstanding request at a time. Frames are UTF-8 JSON terminated by LF, with a 2 MiB maximum including LF. Requests use the JSON-RPC 2.0 envelope with an unsigned integer ID and object `params`; batches and notifications are not supported.

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"version":1,"token":"YOUR_TOKEN"}}
{"jsonrpc":"2.0","id":1,"result":{"version":1,"instance":"OPAQUE_ID","server":"phd"}}
```

Successful unit-returning methods emit `"result":null`. Errors have an `error` object with `code`, `kind`, `message` and `retryable`; the result and error keys are mutually exclusive. IDs echo the request. Wrong credentials/version end the unauthenticated session. Handshake inactivity expires after 5 seconds, authenticated session inactivity after 90 seconds. Keep debug control sessions active by polling `debug.status`. At most 64 socket sessions are accepted concurrently.

`hello.instance` identifies the running daemon. Tasks and cursors belong to that instance. Transport loss leaves the outcome of a submitted mutation uncertain; reconnect explicitly and inspect state. The Rust and Python transports close damaged sessions and never replay calls automatically.

## Operations

Binary inputs/outputs use standard padded base64 unless a method specifies an array of integers. Serial numbers identify devices on the daemon host, independent of the client's OS. `device.open` selects the single board used by subsequent connection-scoped methods; it never silently chooses among multiple probes.

| Method | Parameters / result |
| --- | --- |
| `daemon.health` | Returns version, instance, status and package version |
| `daemon.stop` | Stops sessions, cancels/joins jobs, releases device handles |
| `device.list` | Returns serial, product, USB bus/address, terminal and access error |
| `device.capabilities` | Supported channels and maximum Bulk payload; no selection required |
| `device.open` | Optional exact `serial` or legacy `port`; returns selected serial, terminal, cursor and instance |
| `device.location` | Selected device's physical USB bus/port chain |
| `device.bootloader` | `location`, `product_id`; detects a ROM bootloader on that physical port |
| `board.info` / `board.status` / `board.measure` | Identity string, power status, measurement record respectively |
| `board.power` | `enabled: bool` |
| `board.reset` | `milliseconds: 1..1000` |
| `board.bootsel` | Reboot Debug into its ROM bootloader |
| `console.write` | `data`: at most 128 decoded bytes |
| `console.configure` | `baud`; returns actual baud |
| `console.status` | Returns firmware terminal status tuple |
| `bulk.write` | `route: debug\|work_spi`, `data`: at most 1 MiB; debug route limited to 4096 bytes. Holds coordination/USB/CRC completion as one transaction |
| `logic.start` | `count`: bounded by the selected channel profile (up to 1310720); optional Work GPIO bitmask `channels` (0 = all); `rate`: one of 2500, 5000, 10000, 20000, 50000, 100000, 250000, 500000, 1000000, 2000000, 5000000, 10000000, 20000000, 50000000; `trigger: 0\|1\|2` (none/rising/falling), Work GPIO `pin`, `timeout_ms`; returns connection-owned capture ID |
| `logic.status` | `capture_id`; returns null while running, otherwise `sample_hz`, `count`, nominal `elapsed_us`, and nullable `trigger_index` |
| `logic.read` | `capture_id`, optional nonnegative `offset` (default 0), `limit: 1..32768` (default/max 32768); returns `capture_id`, nominal `elapsed_us` and that slice of chronological Work GPIO words. The client assembles all pages |
| `logic.cancel` | Cancels/resets and joins this connection’s SUMP worker; idempotent; releases the capture lease |
| `events.read` | `cursor`, optional `limit: 1..256`, `wait_ms: 0..1000`; returns instance, cursor, gap and events |
| `ph.request` | Compatibility `data`: serialized PH message fitting a 512-byte firmware frame. Daemon assigns firmware sequence and restores caller sequence in response. Bulk coordination and event submissions are rejected |
| `swd.open` / `swd.disconnect` | Acquire/release the connection's exclusive CMSIS-DAP lease; open returns packet size |
| `swd.connect_swd` | `clock_hz` |
| `swd.connect_swd_adiv6` | `clock_hz`, `ap_base` |
| `swd.connect_swd_core` | `clock_hz`, `core` (0 or 1) |
| `swd.packet_size` / `swd.info` | Negotiated packet size / DAP information for `id` |
| `swd.set_clock` / `swd.swj_sequence` | `hz` / `clocks`, byte array `data` |
| `swd.reset_target` / `swd.set_reset` | `milliseconds` / `asserted` |
| `swd.transfer` | Typed `transfers` array; see `protocol/src/dap.rs` |
| `swd.read_dp` / `swd.write_dp` | `address`, plus `value` for writes |
| `swd.read_ap` / `swd.write_ap` | `address`, plus `value` for writes |
| `swd.read_dp_idcode` / `swd.power_up_debug_port` / `swd.clear_sticky_errors` | DAP connection/recovery primitives |
| `swd.read_memory32` / `swd.write_memory32` | Aligned `address`, `words: 1..16384` / `value` |
| `debug.start` | `serial` matching the selected probe, `target: rp2040\|rp2350`, optional daemon-side `openocd` and `scripts`; selected board supplies serial. Returns daemon-local port |
| `debug.status` / `debug.stop` | Process status/bounded logs / release process and lease |
| `debug.tunnel` | Registered debug `port`; after the success reply the authenticated connection becomes a raw bidirectional GDB stream |
| `upload.begin` | `length: 1..67108864`, `extension: bin\|elf\|uf2`; returns upload ID (at most two per session) |
| `upload.write` | Upload `id`, exact next `offset`, `data` (at most 1 MiB decoded) |
| `upload.finish` | Upload `id`; verifies the declared length was received |
| `task.start` | Completed `upload`, `kind: work_flash\|debug_flash`; Work requires `target`. Optional daemon-side `openocd`, `scripts`, `reset_after`, or `picotool`. Returns task ID and instance |
| `task.list` | Returns retained task IDs, serials, kinds and states; recover after a lost task-start reply |
| `task.status` | Task `id`, optional event `cursor`; returns state, result/error, events, cursor and gap |
| `task.cancel` | Task `id`; requests cancellation, poll for final state |

Power voltage is in mV; measurement fields use shunt uV, bus mV, current mA and power mW. Types are defined in `protocol/src/board.rs` and explicitly described by MCP tools.

## Ownership, events and tasks

A daemon board serializes firmware requests and pumps unsolicited events into a 1024-entry ring. Reading a cursor does not consume another client's events. `device.open` returns the current cursor; cursor 0 inspects retained history. Each event may include raw PH `ph` and decoded Console `console`, both base64. `gap: true` means the caller's history expired or its cursor is outside this ring; resume from the returned cursor and report the loss.

SWD and managed OpenOCD are connection-owned leases. Closing or expiring that connection releases the actual handles/process before releasing ownership. Queries may coexist with SWD; conflicting power/reset/Console/Bulk mutations return `busy`. Programming tasks suspend the shared control handle while their worker owns the device.

Uploads are private to their originating socket and are removed on disconnect unless consumed by `task.start`. Tasks are daemon-owned, survive socket disconnects, and retain at most 128 progress events. At most 64 task records are retained; finished records can be evicted when new tasks start. States are `running`, `succeeded`, `failed`, `cancelled`. Cancellation/shutdown kills and reaps tool subprocesses, releases leases and cleans temporary files. Cancelled programming can leave partial Flash contents; only success confirms verification. Restarting the daemon starts a new task registry.

## Errors

| Kind | Meaning |
| --- | --- |
| `unauthorized` / `version_mismatch` | Credential or socket-version mismatch |
| `invalid_argument` / `invalid_request` / `parse_error` | Invalid method arguments or envelope |
| `device_not_found` / `task_not_found` | Serial/task absent in this daemon |
| `busy` | A competing operation owns a required resource or a bound was reached |
| `device_error` / `file_error` | Device/tool/IO or upload storage failure |
| `method_not_found` / `internal` | Unsupported method / unavailable daemon state |

An error's `retryable` field is advisory and never authorizes automatic repetition of a mutation. Plain TCP provides authentication, not encryption; deployment and service details are in [README.md](README.md).

## Logic acquisition

The daemon selects Logic Analyzer CDC by the selected probe’s USB serial and interface metadata; clients never supply an arbitrary serial-device path. `device.capabilities.logic_capture` advertises RPC support, `logic_transport` is `sump`, and `logic_trigger_requires` names the firmware tag `sump2` needed for corrected trigger-pin routing. Immediate captures work on older SUMP firmware. Rebuild/restart phd as well as the desktop when upgrading from the unsupported capture adapter.

`logic.start` acquires the board lease and returns immediately; polling and measurement queries do not wait for capture completion. Different connections cannot read or cancel one another’s results. Only one capture is retained per connection. Finish/poll or cancel before starting another capture. Disconnect and daemon shutdown cancel/join the worker before releasing ownership. The physical CDC must be free of external SUMP applications.

Capture is finite and board-resident: sampling finishes before USB readout begins. Firmware tag `pc1` enables a 160 KiB buffer, selected-channel packing and verified readout. Selection or counts above the legacy 32768 limit require this tag. All channels hold 40960 points; a single channel holds 1310720 immediate points or 163840 triggered points. Capacity follows physical Debug GPIO span, rounded to 1/2/4/8/16/32 bits per point; triggers use at least 8 bits. The hidden trigger pin is included. See [memory limits and measurements](../doc/logic-memory-validation.md).

The count register remains count/4. Extension register `0x83` accepts a SUMP channel-index mask (0 = all), translated from RPC Work GPIO bits. Flags bit 31 selects packed readout. Immediate samples fill little-endian 32-bit words with the earliest sample in the highest field; triggered samples use individual little-endian 8/16/32-bit transfers. The adapter aligns the wire count to complete words and four-sample batches, then returns exactly the requested count. After readout, command `0x84` returns five little-endian u32 values: magic `0x31504350` (`PCP1`), capture flags, actual count, actual rate and CRC32C of the packed bytes. The adapter rejects stalls, cancellation, timeout, wrong counts and CRC errors. Legacy captures retain four-byte SUMP words without extensions.

Edge capture requires >=8 samples, <=20 MS/s, a connected Work GPIO, and timeout 1..5000 ms; the returned trace must contain the requested edge near the quarter-window trigger position. Failure/timeout is an error. Transport loss never authorizes replay of `logic.start`.

### Raw UART observations (probe firmware `ut1`)

`events.read` can include `uart_trace: {data, tx, baud, timestamp_us, dropped}`.
`data` is base64 for bytes observed at the Debug UART boundary, including PH
headers, CRCs, Hello announcements, baud handshakes and invalid RX bytes.
`tx: true` means bytes accepted by the Debug UART queue; RX means bytes read
from its DMA ring. `timestamp_us` is Debug uptime at observation, not a wire
edge timestamp. `baud` is the actual Debug baud at observation. `dropped` is
the cumulative byte count omitted by the best-effort trace emitter under USB
backpressure. Trace has a separate 8 KiB queue; control messages take priority when starting
a new envelope. Partial envelopes finish before switching queues. This is not
a lossless logic analyzer. The next delivered record reports omissions.
Daemon `gap` remains a separate history overflow signal. Console `console`
events still carry decoded application text and must not be labeled raw UART.

Desktop startup reads `console.status` without changing the rate. Apply calls
`console.configure`; the Debug firmware owns the coordinated Work baud switch.
