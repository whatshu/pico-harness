"""Binary protocol shared with firmware/harness-probe/src/worker.c."""

from dataclasses import dataclass
from enum import IntEnum
import struct

MAGIC = b"PDBG"
VERSION = 1
HEADER = struct.Struct("<4sBBHIHH")
MAX_PAYLOAD = 4096
SPI_TEST_BLOCK_SIZE = 2048
SPI_TEST_HEADER = struct.Struct("<4sIII")


class Command(IntEnum):
    GET_INFO = 1
    GET_STATUS = 2
    POWER_SET = 3
    RESET_PULSE = 4
    BOOTSEL_SET = 5
    ENTER_BOOTSEL = 6
    MEASURE = 7
    GPIO_READ = 8
    CAPTURE = 9
    CAPTURE_READ = 10
    GPIO_DIAG = 11
    GPIO_PULL = 12
    UART_CONFIG = 13
    UART_WRITE = 14
    UART_READ = 15
    UART_STATUS = 16
    SPI_CONFIG = 17
    SPI_TRANSFER = 18
    CAPTURE_STATUS = 19
    DEBUG_ENTER_BOOTSEL = 20
    CAPTURE_START = 21
    CAPTURE_POLL = 22
    CAPTURE_CANCEL = 23


def crc16_ccitt(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
    return crc ^ 0xFFFFFFFF


@dataclass(frozen=True)
class Frame:
    command: int
    sequence: int
    payload: bytes = b""
    flags: int = 0

    def encode(self) -> bytes:
        if len(self.payload) > MAX_PAYLOAD:
            raise ValueError("payload is too large")
        base = HEADER.pack(MAGIC, VERSION, self.command, self.flags, self.sequence, len(self.payload), 0)
        header_crc = crc16_ccitt(base)
        header = HEADER.pack(MAGIC, VERSION, self.command, self.flags, self.sequence, len(self.payload), header_crc)
        return header + self.payload + struct.pack("<I", crc32c(self.payload))

    @classmethod
    def decode(cls, packet: bytes) -> "Frame":
        if len(packet) < HEADER.size + 4:
            raise ValueError("short frame")
        magic, version, command, flags, sequence, length, header_crc = HEADER.unpack_from(packet)
        if magic != MAGIC or version != VERSION:
            raise ValueError("bad magic or version")
        if length > MAX_PAYLOAD:
            raise ValueError("payload is too large")
        if len(packet) != HEADER.size + length + 4:
            raise ValueError("length mismatch")
        zero_crc = HEADER.pack(magic, version, command, flags, sequence, length, 0)
        if crc16_ccitt(zero_crc) != header_crc:
            raise ValueError("header CRC mismatch")
        payload = packet[HEADER.size:-4]
        if crc32c(payload) != struct.unpack_from("<I", packet, len(packet) - 4)[0]:
            raise ValueError("payload CRC mismatch")
        return cls(command, sequence, payload, flags)


def spi_test_pattern(length: int = SPI_TEST_BLOCK_SIZE) -> bytes:
    return bytes((((index * 73 + 41) ^ (index >> 3)) & 0xFF) for index in range(length))


@dataclass(frozen=True)
class SpiSelftestReport:
    blocks: int
    total_errors: int
    last_errors: int

    @classmethod
    def from_transfer(cls, payload: bytes) -> "SpiSelftestReport":
        if len(payload) != SPI_TEST_BLOCK_SIZE:
            raise ValueError(f"expected {SPI_TEST_BLOCK_SIZE} SPI bytes, got {len(payload)}")
        magic, blocks, total_errors, last_errors = SPI_TEST_HEADER.unpack_from(payload)
        if magic != b"PST1":
            raise ValueError("work core SPI selftest is not synchronized")
        expected = spi_test_pattern(len(payload))
        if payload[SPI_TEST_HEADER.size:] != expected[SPI_TEST_HEADER.size:]:
            raise ValueError("work-to-debug SPI data mismatch")
        return cls(blocks, total_errors, last_errors)


@dataclass(frozen=True)
class Measurement:
    shunt_uv: float
    bus_mv: float
    current_ma: float
    power_mw: float

    @classmethod
    def from_payload(cls, payload: bytes) -> "Measurement":
        if not payload:
            raise ValueError("empty measurement response")
        if payload[0]:
            raise RuntimeError(f"probe measurement failed with status {payload[0]}")
        if len(payload) == 17:
            shunt_uv, bus_uv, current_ua, power_uw = struct.unpack("<iiii", payload[1:])
            return cls(shunt_uv, bus_uv / 1000, current_ua / 1000, power_uw / 1000)
        status, shunt, bus, current, power = struct.unpack("<Bhhhh", payload)
        return cls(shunt * 2.5, bus * 1.25, current * 0.1, power * 2.5)


def decode_status(payload: bytes) -> dict:
    if not payload or payload[0]:
        raise RuntimeError("probe status failed")
    if len(payload) not in (4, 7):
        raise ValueError("unexpected status response length")
    status, power_on, power_good, alert = payload[:4]
    result = {"status": status, "power_on": bool(power_on),
              "power_good": bool(power_good), "alert": bool(alert)}
    if len(payload) == 7:
        result.update(power_fault=bool(payload[4]),
                      work_3v3_mv=struct.unpack_from("<H", payload, 5)[0])
    return result
