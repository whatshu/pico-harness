"""Work hardware-test command ordering."""

from pathlib import Path
import unittest
from unittest.mock import patch

from pdbg.worktest import Fixture
from pdbg.worktest_runner import connected_probe


class FakeOpenOCD:
    def __init__(self):
        self.commands = []

    def command(self, command):
        self.commands.append(command)


class WorkFixtureTest(unittest.TestCase):
    def test_versioned_dualcore_probe_is_accepted(self):
        class FakeProbe:
            def command(self, command):
                return b"\0PicoHarness/4;dualcore-v1.2;pico;pdbg-link\0"

            def close(self):
                pass

        probe = FakeProbe()
        with patch("pdbg.worktest_runner.Probe.open", return_value=probe):
            self.assertIs(connected_probe(None, True), probe)

    def test_flash_is_verified_before_ram_fixture_runs(self):
        fixture = object.__new__(Fixture)
        fixture.ocd = FakeOpenOCD()
        fixture.flash_elf = Path("/test/work_selftest.elf")
        fixture.elf = Path("/test/work_selftest_ram.elf")
        fixture.symbols = {"_entry_point": 0x20000179}
        fixture.wait = lambda condition: None
        fixture.words = lambda: [0] * 13 + [0x1c03cc20]

        fixture.load()

        self.assertEqual(fixture.ocd.commands, [
            "program {/test/work_selftest.elf} verify reset",
            "reset halt",
            "load_image {/test/work_selftest_ram.elf}",
            "verify_image {/test/work_selftest_ram.elf}",
            "targets rp2040.core1; resume; targets rp2040.core0",
            "resume 0x20000179",
        ])


if __name__ == "__main__":
    unittest.main()
