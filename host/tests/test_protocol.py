import struct
import unittest

from pdbg.cli import require_ok
from pdbg.device import CaptureResult
from pdbg.logic import DIRECT_GPIO_PINS, analyze_direct_pattern, decode_direct_pattern
from pdbg.protocol import (
    Command,
    Frame,
    MAX_PAYLOAD,
    Measurement,
    SPI_TEST_HEADER,
    SpiSelftestReport,
    crc32c,
    decode_status,
    spi_test_pattern,
)


class ProtocolTest(unittest.TestCase):
    @staticmethod
    def direct_sample(pattern):
        return ((pattern & 0x7FF) << 12) | (((pattern >> 11) & 0x7) << 26)

    @staticmethod
    def direct_patterns(count):
        pattern = 0
        values = []
        for index in range(count):
            values.append(pattern)
            phase = index % 32
            bit = phase % 14 if phase < 28 else (phase - 28) & 1
            pattern ^= 1 << bit
        return values

    def test_maximum_capture_read_response_round_trip(self):
        payload = bytes([0]) + bytes(range(240))
        encoded = Frame(Command.CAPTURE_READ | 0x80, 99, payload).encode()
        self.assertEqual(len(encoded), 16 + 241 + 4)
        self.assertEqual(Frame.decode(encoded).payload, payload)

    def test_maximum_link_payload_round_trip(self):
        payload = bytes((index & 0xFF) for index in range(MAX_PAYLOAD))
        self.assertEqual(Frame.decode(Frame(Command.SPI_TRANSFER, 7, payload).encode()).payload,
                         payload)

    def test_rejects_oversized_link_payload(self):
        with self.assertRaises(ValueError):
            Frame(Command.SPI_TRANSFER, 7, bytes(MAX_PAYLOAD + 1)).encode()

    def test_crc32c_known_vector(self):
        self.assertEqual(crc32c(b"123456789"), 0xE3069283)

    def test_frame_round_trip(self):
        frame = Frame(Command.POWER_SET, 0x12345678, b"\x01")
        self.assertEqual(Frame.decode(frame.encode()), frame)

    def test_rejects_corruption(self):
        packet = bytearray(Frame(Command.GET_INFO, 1).encode())
        packet[-1] ^= 1
        with self.assertRaises(ValueError):
            Frame.decode(packet)

    def test_measurement_scaling(self):
        m = Measurement.from_payload(struct.pack("<Bhhhh", 0, 4, 2640, 123, 40))
        self.assertEqual(m.shunt_uv, 10.0)
        self.assertEqual(m.bus_mv, 3300.0)
        self.assertAlmostEqual(m.current_ma, 12.3)
        self.assertEqual(m.power_mw, 100.0)

    def test_ina237_measurement_units_and_signed_current(self):
        m = Measurement.from_payload(struct.pack("<Biiii", 0, -10, 5_000_000, -200, 1234560))
        self.assertEqual(m.shunt_uv, -10)
        self.assertEqual(m.bus_mv, 5000)
        self.assertEqual(m.current_ma, -0.2)
        self.assertEqual(m.power_mw, 1234.56)

    def test_measurement_device_failure(self):
        with self.assertRaises(RuntimeError):
            Measurement.from_payload(b"\x01")

    def test_status_legacy_and_dualcore(self):
        self.assertEqual(decode_status(b"\0\1\1\0"),
                         dict(status=0, power_on=True, power_good=True, alert=False))
        status = decode_status(struct.pack("<BBBBBH", 0, 0, 1, 0, 1, 3300))
        self.assertFalse(status["power_on"])
        self.assertTrue(status["power_good"])
        self.assertTrue(status["power_fault"])
        self.assertEqual(status["work_3v3_mv"], 3300)

    def test_spi_selftest_report_and_pattern(self):
        payload = bytearray(spi_test_pattern())
        SPI_TEST_HEADER.pack_into(payload, 0, b"PST1", 23, 0, 0)
        self.assertEqual(SpiSelftestReport.from_transfer(payload),
                         SpiSelftestReport(23, 0, 0))

    def test_spi_selftest_rejects_data_corruption(self):
        payload = bytearray(spi_test_pattern())
        SPI_TEST_HEADER.pack_into(payload, 0, b"PST1", 23, 0, 0)
        payload[-1] ^= 1
        with self.assertRaises(ValueError):
            SpiSelftestReport.from_transfer(payload)

    def test_cli_require_ok_returns_successful_payload(self):
        payload = b"\0result"
        self.assertIs(require_ok(payload, "test"), payload)

    def test_cli_require_ok_rejects_error_and_empty_response(self):
        for payload in (b"", b"\x02"):
            with self.subTest(payload=payload), self.assertRaises(RuntimeError):
                require_ok(payload, "test")

    def test_capture_result_reports_measured_frequency(self):
        result = CaptureResult(1, 32768, 512, ())
        self.assertEqual(result.actual_frequency, 64_000_000)

    def test_direct_gpio_pattern_decode_and_sequence(self):
        patterns = self.direct_patterns(32768)
        samples = [self.direct_sample(pattern) for pattern in patterns]
        self.assertEqual(decode_direct_pattern(samples[123]), patterns[123])
        analysis = analyze_direct_pattern(samples, 10_000_000)
        self.assertTrue(analysis.sequence_stable)
        self.assertEqual(analysis.transition_errors, 0)
        self.assertEqual(analysis.invalid_samples, 0)
        self.assertEqual(analysis.inactive_pins, ())

    def test_direct_gpio_pattern_detects_corruption(self):
        samples = [self.direct_sample(pattern) for pattern in self.direct_patterns(100)]
        samples[50] ^= 1 << 20
        analysis = analyze_direct_pattern(samples, 10_000_000)
        self.assertGreater(analysis.invalid_samples, 0)

    def test_direct_gpio_pattern_detects_stuck_and_swapped_pins(self):
        samples = [self.direct_sample(pattern) for pattern in self.direct_patterns(32768)]
        stuck = [sample & ~(1 << 12) for sample in samples]
        self.assertIn(12, analyze_direct_pattern(stuck, 10_000_000).inactive_pins)

        swapped = []
        for sample in samples[:1000]:
            bit_12 = (sample >> 12) & 1
            bit_13 = (sample >> 13) & 1
            swapped.append(sample ^ ((bit_12 ^ bit_13) << 12)
                           ^ ((bit_12 ^ bit_13) << 13))
        self.assertGreater(
            analyze_direct_pattern(swapped, 10_000_000).invalid_samples, 0
        )
        self.assertEqual(len(DIRECT_GPIO_PINS), 14)


if __name__ == "__main__":
    unittest.main()
