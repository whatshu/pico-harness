"""Integration tests for the PH protocol against a connected Debug core.

Requires hardware: runs the PH protocol over the CDC `PH Terminal` (pyserial +
google.protobuf) and the `PH Terminal Bulk` vendor interface (pyusb). Skipped
when no PicoHarness probe is present, so `./build.sh test` stays green offline.
"""

import glob
import sys
import time
import unittest

import usb.core
import usb.util

import phbridge as ph
import phbridge.ph_pb2 as pb

VID, PID = 0x2E8A, 0x000C
BULK_IFNUM = 3  # PH Terminal Bulk


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 & -(crc & 1))
    return (~crc) & 0xFFFFFFFF


def find_terminal() -> str | None:
    for path in sorted(glob.glob("/dev/ttyACM*")):
        import serial

        try:
            s = serial.Serial(path, 115200, timeout=0.15)
        except Exception:
            continue
        msg = pb.PhMessage()
        msg.seq = 1
        msg.get_info.SetInParent()
        s.write(ph.frame(msg))
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


class PhTerminal:
    def __init__(self, port: str):
        import serial

        self.s = serial.Serial(port, 115200, timeout=0.2)
        self.buf = b""
        self.events = []
        self.seq = 0

    def request(self, build, timeout=2.0) -> pb.PhMessage | None:
        self.seq += 1
        msg = pb.PhMessage()
        msg.seq = self.seq
        build(msg)
        self.s.write(ph.frame(msg))
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
                    self.buf = self.buf[1:]  # resync one byte
                    continue
                self.buf = self.buf[1 + ln :]
                if out.seq == self.seq:
                    return out
                if out.seq == 0:  # unsolicited event: hold for wait_event
                    self.events.append(out)
                    continue
        return None

    def wait_event(self, kind: str, timeout=2.0) -> pb.PhMessage | None:
        # Drain events that arrived while a request was awaiting its reply.
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


def b_getinfo(m):
    m.get_info.SetInParent()


def b_getstatus(m):
    m.get_status.SetInParent()


def b_power_on(m):
    m.power_set.enabled = True


def b_power_off(m):
    m.power_set.enabled = False


def b_measure(m):
    m.measure.SetInParent()


def b_console(m):
    m.console_tx.data = b"hello\n"


def b_term(m):
    m.term_config.baud = 115200
    m.term_config.data_bits = 8
    m.term_config.stop_bits = 1
    m.term_config.parity = 0


class PhDeviceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.port = find_terminal()
        if cls.port is None:
            raise unittest.SkipTest("no PicoHarness probe connected (PH Terminal)")

    def setUp(self):
        self.ph = PhTerminal(self.port)

    def test_get_info(self):
        r = self.ph.request(b_getinfo)
        self.assertIsNotNone(r)
        self.assertTrue(r.get_info.info.startswith("PicoHarness/6"), r.get_info.info)

    def test_get_status(self):
        r = self.ph.request(b_getstatus)
        self.assertEqual(r.status, 0)
        self.assertEqual(r.WhichOneof("body"), "get_status")

    def test_power_measure_cycle(self):
        r = self.ph.request(b_power_on)
        self.assertEqual(r.status, 0)
        time.sleep(0.25)
        r = self.ph.request(b_measure)
        self.assertEqual(r.WhichOneof("body"), "measure")
        self.assertGreater(r.measure.current_ua, 0)
        r = self.ph.request(b_power_off)
        self.assertEqual(r.status, 0)

    def test_console_tx_ack(self):
        r = self.ph.request(b_console)
        # OK, or ERR_NOTREADY when the Work rail is unpowered.
        self.assertIn(r.status, (0, 7))

    def test_term_config(self):
        r = self.ph.request(b_term)
        self.assertEqual(r.WhichOneof("body"), "term_config")
        self.assertGreater(r.term_config.actual_baud, 0)

    def test_bulk_data_plane(self):
        payload = bytes(range(64))
        crc = crc32c(payload)

        def b_begin(m):
            m.bulk_tx_begin.route = 0  # ROUTE_DEBUG
            m.bulk_tx_begin.length = len(payload)
            m.bulk_tx_begin.crc32c = crc

        r = self.ph.request(b_begin)
        self.assertEqual(r.status, 0)

        dev = usb.core.find(idVendor=VID, idProduct=PID)
        self.assertIsNotNone(dev)
        itf = None
        for i in dev.get_active_configuration():
            if i.bInterfaceNumber == BULK_IFNUM:
                itf = i
        self.assertIsNotNone(itf, "PH Terminal Bulk interface missing")
        usb.util.claim_interface(dev, BULK_IFNUM)
        try:
            ep_out = usb.util.find_descriptor(
                itf,
                custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
                == usb.util.ENDPOINT_OUT,
            )
            ep_out.write(payload, timeout=2000)
            done = self.ph.wait_event("bulk_tx_done", timeout=2.5)
            self.assertIsNotNone(done)
            self.assertTrue(done.bulk_tx_done.ok, "firmware rejected the payload")
            self.assertEqual(done.bulk_tx_done.crc_recv, crc)
        finally:
            usb.util.release_interface(dev, BULK_IFNUM)

    def test_bulk_work_spi(self):
        """Route WORK_SPI: the Debug core forwards the payload to a harness
        Work core over SPI. Requires a pico_harness Work firmware; skipped
        otherwise (probe for a harness console reply first)."""
        import serial

        # Harness-mode probe: power the rail, let Work announce its idle Hello
        # (mode PLAIN->HARNESS), then a console `info` must visibly reply.
        self.ph.request(b_power_on)
        time.sleep(2.5)  # boot + Hello announcement window

        def b_info(m):
            m.console_tx.data = b"info\n"

        self.ph.request(b_info)
        echoed = self.ph.wait_event("console_data", timeout=3.0)
        if echoed is None:
            self.skipTest("Work core not running a pico_harness console here")

        payload = bytes((i * 7) & 0xFF for i in range(5000))  # multi-block
        crc = crc32c(payload)

        def b_begin(m):
            m.bulk_tx_begin.route = 1  # ROUTE_WORK_SPI
            m.bulk_tx_begin.length = len(payload)
            m.bulk_tx_begin.crc32c = crc

        r = self.ph.request(b_begin)
        self.assertEqual(r.status, 0)

        dev = usb.core.find(idVendor=VID, idProduct=PID)
        itf = None
        for i in dev.get_active_configuration():
            if i.bInterfaceNumber == BULK_IFNUM:
                itf = i
        self.assertIsNotNone(itf)
        usb.util.claim_interface(dev, BULK_IFNUM)
        try:
            ep_out = usb.util.find_descriptor(
                itf,
                custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
                == usb.util.ENDPOINT_OUT,
            )
            ep_out.write(payload, timeout=10000)
            done = self.ph.wait_event("bulk_tx_done", timeout=5.0)
            self.assertIsNotNone(done)
            self.assertTrue(done.bulk_tx_done.ok, "WORK_SPI forwarding rejected")
            self.assertEqual(done.bulk_tx_done.crc_recv, crc)
        finally:
            usb.util.release_interface(dev, BULK_IFNUM)


if __name__ == "__main__":
    unittest.main()