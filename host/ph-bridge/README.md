# ph-bridge — PH Bridge (host)

PC ↔ Debug core connectivity library for PicoHarness, in Rust with Python
bindings (PyO3, `feature = "python"`).

- **PH protocol** (`proto/ph.proto`, package `picoharness.v1`): length-delimited
  protobuf messages over the CDC `PH Terminal` interface. Encode/decode is
  generated with [prost]; the firmware uses [nanopb] generated from the same
  schema.
- **PH Terminal Bulk**: the vendor bulk data plane carries raw payloads; the
  schema only covers the coordination messages (`BulkTxBegin/Done`, ...).

```
host ⇄ Debug : PH protocol (CDC) + PH Terminal Bulk (vendor, raw)
Debug ⇄ Work : pico_harness (UART PH frames + SPI, unchanged) — see doc/protocol.md
```

## Layout

- `src/msg.rs` — generated `picoharness.v1` messages (build.rs runs prost)
- `src/framing.rs` — `<varint length><PhMessage>` splitter/writer
- `src/client.rs` — `PhClient`: request/response + unsolicited-event capture
- `src/device.rs` — Linux sysfs discovery of the control / logic ttyACM ports

## Usage

```rust
use ph_bridge::PhClient;

let mut client = PhClient::open_serial("/dev/ttyACM1")?;
let info = client.get_info()?;             // GetInfo identity string
client.power_set(true)?;                   // TPS22919 rail on
let m = client.measure()?;                 // INA237 shunt/current/power
client.console_tx(b"led on\n")?;           // short console (≤128 B, Work UART)
client.bulk_tx_begin(Route::Debug, len, crc)?; // begin a bulk RX transaction
// ... write payload on the vendor bulk endpoint (0x05 OUT), then wait:
for ev in client.drain_events() { /* BulkTxDone(...) validates crc */ }
```

## Regenerating protobuf code

Firmware `src/ph.pb.{c,h}` and `host/phbridge/ph_pb2.{py}` are checked in;
regenerate from `proto/ph.proto` with (requires `pip install nanopb grpcio-tools`):

```sh
NP=$(python3 -c "import nanopb, os; print(os.path.dirname(nanopb.__file__))")
python3 -m grpc_tools.protoc -I proto -I "$NP/generator/proto" \
  --plugin=protoc-gen-nanopb=$(command -v protoc-gen-nanopb) \
  --nanopb_out=firmware/harness-probe/src proto/ph.proto
python3 -m grpc_tools.protoc -I proto --python_out=host/phbridge proto/ph.proto proto/nanopb.proto
```

The Rust side regenerates at cargo build time via `build.rs` (vendored protoc).

## Tests

```sh
cargo test --manifest-path host/ph-bridge/Cargo.toml          # offline protocol tests
PYTHONPATH=host python3 -m unittest discover -s host/tests    # offline + device (skippable)
./build.sh test --offline                                     # full offline gate
```

[prost]: https://docs.rs/prost
[nanopb]: https://jpa.kapsi.fi/nanopb/