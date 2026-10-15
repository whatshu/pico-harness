import struct
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import call, patch
from pathlib import Path

from pdbg.protocol import Command
from pdbg.device import wait_for_device
from pdbg.update import (
    BOOTSEL_TO_CORE,
    BOOTSEL_USB,
    UF2_FAMILIES,
    debug_image_for_probe,
    probe_core,
    uf2_family,
    update_debug_core,
)


def uf2_block(family, flags=0x2000, address=0x10000000, number=0, count=1):
    block = bytearray(512)
    struct.pack_into("<8I", block, 0, 0x0A324655, 0x9E5D5157, flags, address,
                     256, number, count, family)
    struct.pack_into("<I", block, 508, 0x0AB16F30)
    return bytes(block)


class FakeProbe:
    def __init__(self, info):
        self.info = info
        self.serial = "TEST-SERIAL"
        self.sent = []

    def command(self, command, payload=b"", timeout=2000):
        self.sent.append((int(command), bytes(payload)))
        if int(command) == int(Command.GET_INFO):
            return b"\0" + self.info.encode() + b"\0"
        if int(command) == int(Command.DEBUG_ENTER_BOOTSEL):
            return b"\0"
        raise AssertionError(f"unexpected command {int(command)}")

    def close(self):
        pass


class UpdateTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def write_uf2(self, *blocks):
        path = Path(self.directory.name) / "image.uf2"
        path.write_bytes(b"".join(blocks))
        return path

    def test_uf2_family_reads_both_chip_families(self):
        self.assertEqual(uf2_family(self.write_uf2(uf2_block(UF2_FAMILIES["pico"]))),
                         UF2_FAMILIES["pico"])
        self.assertEqual(uf2_family(self.write_uf2(uf2_block(UF2_FAMILIES["pico2"]))),
                         UF2_FAMILIES["pico2"])

    def test_uf2_family_skips_rp2350_e10_container(self):
        path = self.write_uf2(uf2_block(0xE48BFF57, flags=0xA000),
                              uf2_block(UF2_FAMILIES["pico2"]))
        self.assertEqual(uf2_family(path), UF2_FAMILIES["pico2"])

    def test_uf2_family_rejects_non_uf2_files(self):
        for data in (b"", b"nonsense" * 128, uf2_block(UF2_FAMILIES["pico"])[:-1]):
            path = Path(self.directory.name) / "bad.uf2"
            path.write_bytes(data)
            with self.subTest(prefix=data[:8]), self.assertRaises(ValueError):
                uf2_family(path)

    def test_uf2_family_rejects_family_less_images(self):
        with self.assertRaises(ValueError):
            uf2_family(self.write_uf2(uf2_block(0, flags=0x0)))

    def test_probe_core_extracts_chip_from_info_string(self):
        self.assertEqual(probe_core("PicoHarness/4;dualcore;pico;pdbg-link;uart-dma"), "pico")
        self.assertEqual(probe_core("PicoHarness/4;dualcore;pico2;pdbg-link;uart-dma"), "pico2")

    def test_probe_core_rejects_unknown_core(self):
        with self.assertRaises(RuntimeError):
            probe_core("PicoHarness/4;dualcore;pdbg-link")

    def test_bootsel_usb_pid_matches_chip_family(self):
        self.assertEqual(BOOTSEL_USB["pico"], (0x2E8A, 0x0003))
        self.assertEqual(BOOTSEL_USB["pico2"], (0x2E8A, 0x000F))
        self.assertEqual(BOOTSEL_TO_CORE, {0x0003: "pico", 0x000F: "pico2"})

    def test_update_refuses_wrong_family_before_rebooting_the_probe(self):
        probe = FakeProbe("PicoHarness/4;dualcore;pico;pdbg-link")
        pico2_image = self.write_uf2(uf2_block(UF2_FAMILIES["pico2"]))
        with self.assertRaises(SystemExit):
            update_debug_core(probe, pico2_image, picotool="picotool")
        self.assertEqual([command for command, _ in probe.sent],
                         [int(Command.GET_INFO)])

    def test_update_waits_for_correct_products_and_pins_picotool_to_debug_usb(self):
        probe = FakeProbe("PicoHarness/4;dualcore;pico;pdbg-link")
        image = self.write_uf2(uf2_block(UF2_FAMILIES["pico"]))
        loader = SimpleNamespace(bus=7, address=9)
        with patch("pdbg.update.wait_for_device", autospec=wait_for_device,
                   side_effect=[loader, object()]) as wait, \
                patch("pdbg.update.Probe.open", return_value=probe), \
                patch("pdbg.update.subprocess.run") as run:
            update_debug_core(probe, image, log=lambda _: None)
        self.assertEqual(wait.call_args_list, [
            call(0x0003, timeout=10.0),
            call(0x000C, serial="TEST-SERIAL", timeout=20.0),
        ])
        self.assertEqual(run.call_args_list, [
            call(["picotool", "load", "-v", "--bus", "7", "--address", "9",
                  "--family", "0xe48bff56", str(image)], check=True),
            call(["picotool", "reboot", "--bus", "7", "--address", "9"], check=True),
        ])

    def test_flash_selects_the_rp2040_dualcore_image(self):
        artifacts = Path(self.directory.name)
        image = artifacts / "pico_dualcore_debug_pico.uf2"
        image.write_bytes(uf2_block(UF2_FAMILIES["pico"]))
        probe = FakeProbe("PicoHarness/4;dualcore-v1.2;pico;pdbg-link")
        self.assertEqual(debug_image_for_probe(probe, artifacts), image)

    def test_flash_rejects_an_rp2350_debug_core(self):
        artifacts = Path(self.directory.name)
        (artifacts / "pico_dualcore_debug_pico2.uf2").write_bytes(
            uf2_block(UF2_FAMILIES["pico2"]))
        probe = FakeProbe("PicoHarness/4;dualcore;pico2;pdbg-link")
        with self.assertRaisesRegex(RuntimeError, "must be RP2040"):
            debug_image_for_probe(probe, artifacts)

    def test_flash_rejects_a_different_debug_board(self):
        probe = FakeProbe("PicoHarness/4;v1.2;pico;pdbg-link")
        with self.assertRaisesRegex(RuntimeError, "not a dualcore Debug core"):
            debug_image_for_probe(probe, self.directory.name)


if __name__ == "__main__":
    unittest.main()
