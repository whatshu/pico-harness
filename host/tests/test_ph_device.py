"""Opt-in hardware tests through phd, including remote daemon hosts.

Set PHD_HARDWARE_TEST=1 and PHD_SERIAL to a dedicated fixture. The power test
restores the initial rail state even when a measurement fails.
"""
import os
import unittest

from phbridge.client import PhClient
from phbridge.ipc import Client


@unittest.skipUnless(os.environ.get("PHD_HARDWARE_TEST") == "1", "hardware tests require PHD_HARDWARE_TEST=1")
class PhDeviceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.serial = os.environ.get("PHD_SERIAL")
        if not cls.serial:
            raise RuntimeError("PHD_SERIAL is required for hardware tests")

    def setUp(self):
        self.ph = PhClient(self.serial)
        self.addCleanup(self.ph.close)

    def test_info_and_status(self):
        info = self.ph.get_info()
        self.assertEqual(info.status, 0)
        self.assertTrue(info.get_info.info.startswith("PicoHarness/"))
        self.assertEqual(self.ph.get_status().WhichOneof("body"), "get_status")

    def test_measurement_and_power_restore(self):
        initial = self.ph.get_status().get_status.power_enabled
        try:
            self.assertEqual(self.ph.power_set(True).status, 0)
            self.assertEqual(self.ph.measure().WhichOneof("body"), "measure")
        finally:
            self.assertEqual(self.ph.power_set(initial).status, 0)

    def test_terminal_configuration(self):
        reply = self.ph.term_config(115200)
        self.assertEqual(reply.status, 0)
        self.assertGreater(reply.term_config.actual_baud, 0)

    def test_bulk_transaction(self):
        # Debug route is bounded and does not forward arbitrary SPI to Work.
        result = self.ph.bulk_write(bytes(range(64)), route="debug")
        self.assertTrue(result["ok"])
        self.assertEqual(result["bytes"], 64)

    def test_independent_clients(self):
        with Client() as other:
            other.call("device.open", {"serial": self.serial})
            self.assertEqual(other.call("board.info"), self.ph.get_info().get_info.info)
