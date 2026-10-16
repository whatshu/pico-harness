"""PH protocol client over a serial port (pyserial). Pure Python so the `ph`
CLI and scripts work without building the Rust crate.

See `proto/ph.proto` for the wire format (length-delimited protobuf).
"""

from __future__ import annotations

import time

import phbridge as ph

import phbridge.ph_pb2 as pb


class PhClient:
    """Request/response over the CDC `PH Terminal` serial port."""

    def __init__(self, port: str, timeout: float = 0.2):
        import serial

        self.s = serial.Serial(port, 115200, timeout=timeout)
        self.buf = b""
        self.events: list = []
        self.seq = 0
        self._tmo = timeout

    def close(self) -> None:
        self.s.close()

    def request(self, build, request_timeout: float = 2.0) -> pb.PhMessage | None:
        self.seq += 1
        msg = pb.PhMessage()
        msg.seq = self.seq
        build(msg)
        self.s.write(ph.frame(msg))
        deadline = time.time() + request_timeout
        while time.time() < deadline:
            chunk = self.s.read(512)
            if not chunk:
                continue
            self.buf += chunk
            while True:
                if not self.buf:
                    break
                ln = self.buf[0]
                if len(self.buf) < 1 + ln:
                    break
                try:
                    out = pb.PhMessage()
                    out.ParseFromString(self.buf[1 : 1 + ln])
                except Exception:
                    self.buf = self.buf[1:]
                    continue
                self.buf = self.buf[1 + ln :]
                if out.seq == self.seq:
                    return out
                if out.seq == 0:
                    self.events.append(out)
        return None

    def wait_event(self, kind: str, timeout: float = 2.0) -> pb.PhMessage | None:
        for i, ev in enumerate(self.events):
            if ev.WhichOneof("body") == kind:
                return self.events.pop(i)
        deadline = time.time() + timeout
        while time.time() < deadline:
            chunk = self.s.read(512)
            if not chunk:
                continue
            self.buf += chunk
            while True:
                if not self.buf:
                    break
                ln = self.buf[0]
                if len(self.buf) < 1 + ln:
                    break
                try:
                    out = pb.PhMessage()
                    out.ParseFromString(self.buf[1 : 1 + ln])
                except Exception:
                    self.buf = self.buf[1:]
                    continue
                self.buf = self.buf[1 + ln :]
                if out.WhichOneof("body") == kind:
                    return out
                if out.seq == 0:
                    self.events.append(out)
        return None

    def drain_events(self) -> list:
        ev = self.events
        self.events = []
        return ev

    # ---- friendly helpers -------------------------------------------------

    def get_info(self) -> pb.PhMessage | None:
        return self.request(lambda m: m.get_info.SetInParent())

    def get_status(self) -> pb.PhMessage | None:
        return self.request(lambda m: m.get_status.SetInParent())

    def power_set(self, enabled: bool) -> pb.PhMessage | None:
        return self.request(lambda m: setattr(m.power_set, "enabled", enabled))

    def measure(self) -> pb.PhMessage | None:
        return self.request(lambda m: m.measure.SetInParent())

    def console_tx(self, data: bytes) -> pb.PhMessage | None:
        return self.request(lambda m: setattr(m.console_tx, "data", data))

    def term_config(self, baud: int) -> pb.PhMessage | None:
        def build(m):
            m.term_config.baud = baud
            m.term_config.data_bits = 8
            m.term_config.stop_bits = 1
            m.term_config.parity = 0

        return self.request(build)

    def bulk_tx_begin(self, route: int, length: int, crc: int) -> pb.PhMessage | None:
        def build(m):
            m.bulk_tx_begin.route = route
            m.bulk_tx_begin.length = length
            m.bulk_tx_begin.crc32c = crc

        return self.request(build)

    def debug_bootsel(self) -> pb.PhMessage | None:
        return self.request(lambda m: m.debug_bootsel.SetInParent())


def scan_terminals():
    """Return the first ttyACM that answers a GetInfo over the PH protocol."""
    import glob

    import serial

    for path in sorted(glob.glob("/dev/ttyACM*")):
        try:
            s = serial.Serial(path, 115200, timeout=0.15)
        except Exception:
            continue
        m = pb.PhMessage()
        m.seq = 1
        m.get_info.SetInParent()
        s.write(ph.frame(m))
        t0 = time.time()
        while time.time() - t0 < 0.4:
            chunk = s.read(128)
            if chunk and len(chunk) > 1:
                try:
                    r = pb.PhMessage()
                    r.ParseFromString(chunk[1 : 1 + chunk[0]])
                    if r.WhichOneof("body") == "get_info":
                        s.close()
                        return path
                except Exception:
                    pass
        s.close()
    return None