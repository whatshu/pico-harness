"""Run production CRC, SPI, Bulk and SUMP code against hardware adapters."""
import binascii
import ctypes
import struct
import unittest

from test_dualcore_board import FirmwareBoardModel
from tools.check_bridge import crc32c


class ProbeTransportChecks(FirmwareBoardModel):
    extra_sources = ("src/ph_bulk.c", "src/ph_sump.c", "tests/transport_stub.c")

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.lib.ph_fw_target_spi_start.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint16]
        cls.lib.ph_fw_target_spi_start.restype = ctypes.c_bool
        cls.lib.ph_fw_crc32c.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        cls.lib.ph_fw_crc32c.restype = ctypes.c_uint32
        cls.lib.ph_fw_crc32c_update.argtypes = [ctypes.c_uint32, ctypes.c_void_p, ctypes.c_size_t]
        cls.lib.ph_fw_crc32c_update.restype = ctypes.c_uint32
        cls.lib.ph_bulk_rx_begin.argtypes = [ctypes.c_uint32] * 3
        cls.lib.ph_bulk_rx_begin.restype = ctypes.c_bool
        cls.lib.ph_bulk_busy.restype = ctypes.c_bool
        cls.lib.ph_bulk_on_uart.argtypes = [ctypes.c_uint8, ctypes.c_uint16,
                                          ctypes.c_void_p, ctypes.c_size_t]

    def setUp(self):
        self.lib.ph_fw_target_spi_cancel()
        super().setUp()
        self.lib.mock_transport_reset()
        self.lib.ph_bulk_init()
        self.lib.ph_sump_init()
        self.tx = ctypes.create_string_buffer(bytes(range(64)))
        self.rx = ctypes.create_string_buffer(64)

    def start(self):
        return self.lib.ph_fw_target_spi_start(self.tx, self.rx, 64)

    def test_spi_rejects_unpowered_empty_and_overlapping_transfers(self):
        self.assertFalse(self.start())
        self.lib.ph_fw_target_io_set_ready(True)
        self.assertFalse(self.lib.ph_fw_target_spi_start(self.tx, self.rx, 0))
        self.assertTrue(self.start())
        self.assertFalse(self.start())
        self.lib.ph_fw_target_spi_cancel()
        self.assertEqual(self.lib.ph_fw_target_spi_poll(), -1)
        self.assertEqual(self.array('levels')[9], 1)
        self.assertTrue(self.start())

    def test_spi_returns_while_dma_runs_and_waits_for_wire_idle(self):
        self.lib.ph_fw_target_io_set_ready(True)
        self.var('mock_dma_busy').value = 3  # SPI RX/TX are the first channels.
        self.assertTrue(self.start())
        self.assertEqual(self.array('levels')[9], 0)
        for _ in range(100):
            self.assertEqual(self.lib.ph_fw_target_spi_poll(), 0)
        self.assertEqual(self.var('now_us').value, 0)
        self.var('mock_dma_busy').value = 0
        self.var('mock_spi_busy').value = 1
        self.assertEqual(self.lib.ph_fw_target_spi_poll(), 0)
        self.var('mock_spi_busy').value = 0
        self.assertEqual(self.lib.ph_fw_target_spi_poll(), 1)
        self.assertEqual(self.array('levels')[9], 1)
        self.assertEqual(self.lib.ph_fw_target_spi_poll(), -1)

    def test_spi_cancels_dma_on_timeout_and_power_loss(self):
        for reason in ('timeout', 'power'):
            with self.subTest(reason=reason):
                self.lib.ph_fw_target_io_set_ready(True)
                self.var('mock_dma_busy').value = 3
                self.var('mock_dma_aborts').value = 0
                self.assertTrue(self.start())
                if reason == 'timeout':
                    self.var('now_us').value += 5_000_001
                else:
                    self.lib.ph_fw_target_io_set_ready(False)
                self.assertEqual(self.lib.ph_fw_target_spi_poll(), -1)
                self.assertEqual(self.var('mock_dma_busy').value, 0)
                self.assertEqual(self.var('mock_dma_aborts').value, 3)
                self.assertEqual(self.array('levels')[9], 1)

    def test_crc32c_known_vector_empty_and_incremental_splits(self):
        vector = ctypes.create_string_buffer(b'123456789')
        self.assertEqual(self.lib.ph_fw_crc32c(vector, 9), 0xe3069283)
        self.assertEqual(self.lib.ph_fw_crc32c(None, 0), 0)
        data = ctypes.create_string_buffer(bytes(range(256)) * 16)
        expected = self.lib.ph_fw_crc32c(data, 4096)
        for split in (0, 1, 63, 64, 65, 2048, 4095, 4096):
            with self.subTest(split=split):
                crc = self.lib.ph_fw_crc32c_update(0xffffffff, data, split)
                crc = self.lib.ph_fw_crc32c_update(crc, ctypes.byref(data, split), 4096 - split)
                self.assertEqual(crc ^ 0xffffffff, expected)

    def feed_vendor(self, data):
        ctypes.memmove((ctypes.c_uint8 * 8192).in_dll(self.lib, 'mock_vendor_input'), data, len(data))
        self.var('mock_vendor_size').value = len(data)
        self.var('mock_vendor_pos').value = 0
        self.lib.ph_bulk_pump_rx()

    def begin_spi(self, data):
        self.lib.ph_fw_target_io_set_ready(True)
        self.assertTrue(self.lib.ph_bulk_rx_begin(1, len(data), crc32c(data)))
        self.feed_vendor(data)
        for _ in range(3):
            self.lib.ph_bulk_pump_rx()
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_uart_kind').value, 6)
        length = self.var('mock_uart_length').value
        self.lib.ph_bulk_on_uart(0x86, 0, struct.pack('<H', length), 2)
        self.var('mock_dma_busy').value = 3
        self.lib.ph_bulk_worker()
        self.assertEqual(self.array('levels')[9], 0)

    def test_bulk_accepts_uart_crc_before_dma_poll(self):
        data = bytes(range(128)) * 16
        self.begin_spi(data)
        crc = struct.pack('<H', binascii.crc_hqx(data, 0xffff))
        self.lib.ph_bulk_on_uart(0x87, 0, crc, 2)
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_count').value, 0)
        self.var('mock_dma_busy').value = 0
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_count').value, 1)
        self.assertEqual(self.var('mock_done_ok').value, 1)
        self.assertEqual(self.var('mock_done_crc').value, crc32c(data))
        self.assertFalse(self.lib.ph_bulk_busy())

    def test_bulk_multiple_spi_blocks_wrap_ring(self):
        prefix = bytes(3000)
        self.assertTrue(self.lib.ph_bulk_rx_begin(0, len(prefix), crc32c(prefix)))
        self.feed_vendor(prefix)
        self.lib.ph_bulk_worker()
        self.var('mock_done_count').value = 0
        data = bytes((i * 37 + 19) & 255 for i in range(5000))
        self.begin_spi(data)
        for offset in range(0, len(data), 2048):
            block = data[offset:offset + 2048]
            self.assertEqual(self.var('mock_uart_length').value, len(block))
            self.lib.ph_bulk_on_uart(0x87, 0, struct.pack('<H', binascii.crc_hqx(block, 0xffff)), 2)
            self.var('mock_dma_busy').value = 0
            self.lib.ph_bulk_worker()
            if offset + len(block) < len(data):
                for _ in range(2):
                    self.lib.ph_bulk_pump_rx()
                self.lib.ph_bulk_worker()
                length = self.var('mock_uart_length').value
                self.lib.ph_bulk_on_uart(0x86, 0, struct.pack('<H', length), 2)
                self.var('mock_dma_busy').value = 3
                self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_count').value, 1)
        self.assertEqual(self.var('mock_done_ok').value, 1)
        self.assertEqual(self.var('mock_done_crc').value, crc32c(data))

    def test_bulk_power_loss_emits_one_failure_and_releases_transaction(self):
        self.begin_spi(bytes(64))
        self.lib.ph_fw_target_io_set_ready(False)
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_count').value, 1)
        self.assertEqual(self.var('mock_done_ok').value, 0)
        self.assertFalse(self.lib.ph_bulk_busy())
        self.assertEqual(self.array('levels')[9], 1)
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_count').value, 1)

    def test_bulk_reset_defers_dma_cleanup_and_rejects_stale_crc(self):
        data = bytes(range(64))
        self.begin_spi(data)
        self.lib.ph_bulk_reset()
        self.assertTrue(self.lib.ph_bulk_busy())
        self.assertFalse(self.lib.ph_bulk_rx_begin(0, len(data), crc32c(data)))
        self.assertEqual(self.var('mock_dma_busy').value, 3)
        self.lib.ph_bulk_on_uart(0x87, 0, struct.pack('<H', binascii.crc_hqx(data, 0xffff)), 2)
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_dma_busy').value, 0)
        self.assertEqual(self.array('levels')[9], 1)
        self.assertEqual(self.var('mock_done_count').value, 0)
        self.assertFalse(self.lib.ph_bulk_busy())
        self.assertTrue(self.lib.ph_bulk_rx_begin(0, len(data), crc32c(data)))
        self.feed_vendor(data)
        self.lib.ph_bulk_worker()
        self.assertEqual(self.var('mock_done_ok').value, 1)

    def test_bulk_ring_wrap_and_crc_rejection(self):
        for length, valid in ((3000, True), (4096, True), (65, False), (4096, True)):
            with self.subTest(length=length, valid=valid):
                data = bytes((i * 73 + 41) & 255 for i in range(length))
                expected = crc32c(data)
                self.assertTrue(self.lib.ph_bulk_rx_begin(0, length, expected if valid else expected ^ 1))
                # Two pumps may be needed at the ring wrap.
                self.feed_vendor(data)
                self.lib.ph_bulk_pump_rx()
                self.lib.ph_bulk_worker()
                self.assertFalse(self.lib.ph_bulk_busy())
                self.assertEqual(self.var('mock_done_ok').value, valid)
                self.assertEqual(self.var('mock_done_crc').value, expected)

    def feed_cdc(self, data):
        ctypes.memmove((ctypes.c_uint8 * 128).in_dll(self.lib, 'mock_cdc_input'), data, len(data))
        self.var('mock_cdc_size').value = len(data)
        self.var('mock_cdc_pos').value = 0
        self.lib.ph_sump_pump()

    def test_sump_retains_each_id_reply_under_backpressure(self):
        self.var('mock_cdc_space').value = 3
        self.feed_cdc(b'\x02\x02\x02')
        self.assertEqual(self.var('mock_cdc_used').value, 0)
        for _ in range(3):
            self.var('mock_cdc_space').value = 64
            self.lib.ph_sump_pump()
        output = (ctypes.c_uint8 * 12).in_dll(self.lib, 'mock_cdc_output')
        self.assertEqual(bytes(output), b'1ALS' * 3)
        self.lib.ph_sump_pump()
        self.assertEqual(self.var('mock_cdc_writes').value, 3)

    def test_sump_batches_samples_preserves_channel_order_and_padding(self):
        self.feed_cdc(b'\x81' + struct.pack('<I', 5) + b'\x01')  # 20 samples
        self.lib.ph_sump_worker()
        self.assertEqual(self.var('mock_la_count').value, 20)
        self.var('mock_la_count').value = 18  # last two samples must be padded
        self.var('mock_la_complete').value = 1
        self.lib.ph_sump_worker()
        self.var('mock_cdc_space').value = 3
        self.lib.ph_sump_pump()
        self.assertEqual(self.var('mock_cdc_writes').value, 0)
        self.var('mock_cdc_space').value = 64
        self.lib.ph_sump_pump()
        self.assertEqual(self.var('mock_cdc_used').value, 64)
        self.assertEqual(self.var('mock_cdc_writes').value, 1)
        self.assertEqual(self.var('mock_cdc_flushes').value, 0)
        self.var('mock_cdc_space').value = 64
        self.lib.ph_sump_pump()
        self.assertEqual(self.var('mock_cdc_used').value, 80)
        self.assertEqual(self.var('mock_cdc_writes').value, 2)
        self.assertEqual(self.var('mock_cdc_flushes').value, 1)
        output = bytes((ctypes.c_uint8 * 80).in_dll(self.lib, 'mock_cdc_output'))
        # Preserve the existing SUMP wire order, including the carrier routing.
        pins = [28, 27, 26, 5, 17, 16, 11, 10, 15, 14, 13, 12, 19, 21, 18, 20]
        expected = []
        for index in range(20):
            sample = ((index * 0x12345) ^ 0x1c3ffc20) if index < 18 else 0
            expected.append(sum(((sample >> pin) & 1) << bit for bit, pin in enumerate(pins)))
        self.assertEqual(struct.unpack('<20I', output), tuple(expected))
        self.lib.ph_sump_pump()
        self.assertEqual(self.var('mock_cdc_writes').value, 2)


class ProbePicoTransportTest(ProbeTransportChecks, unittest.TestCase):
    rp2040 = 1


class ProbePico2TransportTest(ProbeTransportChecks, unittest.TestCase):
    rp2040 = 0
