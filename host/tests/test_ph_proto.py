"""Offline unit tests for the PH protocol framing and wire compatibility.

No hardware required. `./build.sh test` discovers these via host/tests.
Golden vectors below were captured from the nanopb-generated firmware encoder,
so this also pins nanopb (C) <-> python wire compatibility.
"""

import unittest

import phbridge as ph
import phbridge.ph_pb2 as pb


class VarintTests(unittest.TestCase):
    def test_roundtrip(self):
        for value in (0, 1, 127, 128, 300, 4096, 9024):
            enc = ph.varint_encode(value)
            self.assertEqual(ph.varint_decode(enc)[0], value)
            self.assertEqual(ph.varint_decode(enc)[1], len(enc))

    def test_multi_byte(self):
        self.assertEqual(ph.varint_encode(9024).hex(), "c046")
        self.assertEqual(ph.varint_encode(128).hex(), "8001")


class FramingTests(unittest.TestCase):
    def test_golden_wire_bytes(self):
        """Must match nanopb pb_encode_delimited on identical messages."""
        power = pb.PhMessage()
        power.seq = 7
        power.power_set.enabled = True
        self.assertEqual(ph.frame(power).hex(), "0708079201020801")

        console = pb.PhMessage()
        console.seq = 9
        console.console_tx.data = b"hi"
        self.assertEqual(ph.frame(console).hex(), "0908098202040a026869")

    def test_split_anywhere(self):
        msg = pb.PhMessage()
        msg.seq = 1
        msg.get_info.SetInParent()
        raw = ph.frame(msg)
        r = ph.FrameReader()
        for b in raw:
            r.feed(bytes([b]))
        out = r.next_message()
        self.assertIsNotNone(out)
        self.assertEqual(out.seq, 1)
        self.assertEqual(out.WhichOneof("body"), "get_info")

    def test_bad_prefix_resyncs(self):
        r = ph.FrameReader()
        msg = pb.PhMessage()
        msg.seq = 3
        r.feed(b"\xe8\x46")  # bogus length prefix (2-byte varint)
        r.feed(ph.frame(msg))
        out = r.next_message()
        self.assertIsNotNone(out)
        self.assertEqual(out.seq, 3)

    def test_known_hex(self):
        raw = bytes.fromhex("020803")
        r = ph.FrameReader()
        r.feed(raw)
        out = r.next_message()
        self.assertEqual(out.seq, 3)


if __name__ == "__main__":
    unittest.main()