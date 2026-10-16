"""PH (Pico Harness) protocol framing helpers shared by tests and tooling.

Wire format: length-delimited protobuf, `<varint length><PhMessage bytes>`,
identical to nanopb's `pb_encode_delimited` and prost's
`encode_length_delimited`. This module is dependency-light (google.protobuf
only) so `./build.sh test` can exercise the protocol offline.
"""

from __future__ import annotations

import os
import sys

# The generated `ph_pb2.py` does a bare `import nanopb_pb2`; the sibling module
# must be importable even when only the parent dir is on sys.path.
_here = os.path.dirname(os.path.abspath(__file__))
if _here not in sys.path:
    sys.path.insert(0, _here)


def varint_encode(value: int) -> bytes:
    assert value >= 0
    out = bytearray()
    while True:
        b = value & 0x7F
        value >>= 7
        if value:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def varint_decode(data: bytes, pos: int = 0) -> tuple[int, int]:
    """Return (value, bytes_consumed); raises ValueError on overflow (>5)."""
    value = 0
    for i in range(5):
        if pos + i >= len(data):
            raise ValueError("incomplete varint")
        b = data[pos + i]
        value |= (b & 0x7F) << (7 * i)
        if not b & 0x80:
            return value, i + 1
    raise ValueError("varint exceeds 5 bytes")


def frame(message) -> bytes:
    """Length-delimited frame for a protobuf message."""
    body = message.SerializeToString()
    return varint_encode(len(body)) + body


class FrameReader:
    """Incremental splitter for a length-delimited PhMessage stream."""

    def __init__(self):
        self.buf = b""

    def feed(self, data: bytes) -> None:
        self.buf += data

    def next_message(self):
        """Pop one PhMessage, or None when more bytes are required. Skips
        corrupt/oversized prefixes so the stream resyncs automatically."""
        import phbridge.ph_pb2 as pb

        while True:
            if not self.buf:
                return None
            try:
                length, n = varint_decode(self.buf)
            except ValueError:
                # Incomplete/malformed varint: drop one byte and resync.
                self.buf = self.buf[1:]
                continue
            if length == 0 or length > 512:  # mirrors firmware PH_MSG_MAX
                self.buf = self.buf[n:]
                continue
            total = n + length
            if len(self.buf) < total:
                return None
            body = self.buf[n:total]
            self.buf = self.buf[total:]
            msg = pb.PhMessage()
            msg.ParseFromString(body)
            return msg