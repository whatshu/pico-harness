# nanopb runtime (generator artifacts)

Nanopb 0.4.9.1 (Zlib license, see LICENSE-notice below). Runtime C sources only —
the protoc plugin / generator is NOT vendored; generated `ph.pb.{c,h}` under
`firmware/harness-probe/src/` are checked in, so firmware builds need neither
protoc nor nanopb.

Source: https://github.com/nanopb/nanopb (tag 0.4.9.1). License: Zlib
(see pb.h header and https://jpa.kapsi.fi/nanopb/).

To regenerate `ph.pb.{c,h}` (requires pip `nanopb` + `grpcio-tools`):
```
NP=$(python3 -c "import nanopb, os; print(os.path.dirname(nanopb.__file__))")
python3 -m grpc_tools.protoc -I proto -I "$NP/generator/proto" \
  --plugin=protoc-gen-nanopb=$(command -v protoc-gen-nanopb) \
  --nanopb_out=firmware/harness-probe/src proto/ph.proto
```
