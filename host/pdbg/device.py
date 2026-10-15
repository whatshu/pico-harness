"""PyUSB transport. Importing this module does not require a connected board."""

from dataclasses import dataclass
from itertools import count
import struct
import time
from .protocol import Command, Frame, HEADER, MAX_PAYLOAD


@dataclass(frozen=True)
class UartStatus:
    rx_available: int
    tx_space: int
    rx_overruns: int


@dataclass(frozen=True)
class CaptureResult:
    capture_id: int
    count: int
    elapsed_us: int
    samples: tuple[int, ...]

    @property
    def actual_frequency(self):
        return self.count * 1_000_000 / self.elapsed_us


class Probe:
    def __init__(self, device, interface, endpoint_out, endpoint_in):
        self.device = device
        self.interface = interface
        self.endpoint_out = endpoint_out
        self.endpoint_in = endpoint_in
        self._sequence = count(1)
        self.serial = None

    @classmethod
    def open(cls, serial=None):
        import usb.core
        import usb.util

        try:
            devices = usb.core.find(find_all=True, idVendor=0x2E8A, idProduct=0x000C)
        except usb.core.NoBackendError as error:
            raise RuntimeError("PyUSB libusb backend is unavailable") from error
        for device in devices:
            if serial and usb.util.get_string(device, device.iSerialNumber) != serial:
                continue
            configuration = device.get_active_configuration()
            for interface in configuration:
                if (interface.bInterfaceClass == 0xFF and
                        interface.bInterfaceSubClass == 0x50 and
                        interface.bInterfaceProtocol == 0x01):
                    endpoints = list(interface)
                    out_ep = next(ep for ep in endpoints if usb.util.endpoint_direction(ep.bEndpointAddress) == usb.util.ENDPOINT_OUT)
                    in_ep = next(ep for ep in endpoints if usb.util.endpoint_direction(ep.bEndpointAddress) == usb.util.ENDPOINT_IN)
                    probe = cls(device, interface, out_ep, in_ep)
                    probe.serial = usb.util.get_string(device, device.iSerialNumber)
                    return probe
        raise RuntimeError("PDBG Link bulk interface not found")

    def close(self):
        """Release the claimed USB interface so re-enumeration stays clean."""
        import usb.util
        if self.device is not None:
            usb.util.dispose_resources(self.device)
            self.device = None

    def command(self, command, payload=b"", timeout=2000):
        sequence = next(self._sequence)
        request = Frame(command, sequence, payload).encode()
        self.endpoint_out.write(request, timeout)
        raw = bytes(self.endpoint_in.read(HEADER.size + MAX_PAYLOAD + 4, timeout))
        response = Frame.decode(raw)
        if response.sequence != sequence or response.command != (int(command) | 0x80):
            raise RuntimeError("response does not match request")
        return response.payload

    @staticmethod
    def _ok(payload, action):
        if not payload:
            raise RuntimeError(f"{action} failed: empty response")
        if payload[0]:
            raise RuntimeError(f"{action} failed with status {payload[0]}")
        return payload[1:]

    def reset(self, milliseconds=10):
        self._ok(self.command(Command.RESET_PULSE, struct.pack("<H", milliseconds)),
                 "pulse reset")

    def uart_config(self, channel, baud, data_bits=8, stop_bits=1, parity=0):
        payload = struct.pack("<BIBBB", channel, baud, data_bits, stop_bits, parity)
        response = self._ok(self.command(Command.UART_CONFIG, payload), "configure UART")
        return struct.unpack("<I", response)[0]

    def uart_status(self, channel):
        response = self._ok(self.command(Command.UART_STATUS, bytes([channel])),
                            "read UART status")
        return UartStatus(*struct.unpack("<HHI", response))

    def uart_write(self, channel, data, timeout=2.0):
        data = bytes(data)
        written = 0
        deadline = time.monotonic() + timeout
        while written < len(data):
            chunk = data[written:written + MAX_PAYLOAD - 1]
            response = self._ok(
                self.command(Command.UART_WRITE, bytes([channel]) + chunk),
                "write UART",
            )
            accepted = struct.unpack("<H", response)[0]
            written += accepted
            if accepted == 0:
                if time.monotonic() >= deadline:
                    raise TimeoutError("UART transmit ring stayed full")
                time.sleep(0.001)
        return written

    def uart_read(self, channel, maximum=MAX_PAYLOAD - 1):
        if not 0 <= maximum <= MAX_PAYLOAD - 1:
            raise ValueError("UART read size is out of range")
        response = self.command(Command.UART_READ, struct.pack("<BH", channel, maximum))
        return self._ok(response, "read UART")

    def capture(self, samples, frequency):
        if not 1 <= samples <= 32768:
            raise ValueError("capture size must be 1..32768 samples")
        if frequency <= 0:
            raise ValueError("capture frequency must be positive")
        timeout = max(5000, int(samples * 2000 / frequency) + 2000)
        response = self._ok(
            self.command(Command.CAPTURE, struct.pack("<II", samples, frequency),
                         timeout=timeout),
            "capture GPIO",
        )
        if len(response) != 12:
            raise RuntimeError("capture response does not include timing data")
        capture_id, captured, elapsed_us = struct.unpack("<III", response)
        if captured != samples or elapsed_us == 0:
            raise RuntimeError("capture returned invalid sample or timing data")

        values = []
        offset = 0
        while offset < captured:
            chunk = min(1023, captured - offset)
            payload = self._ok(
                self.command(Command.CAPTURE_READ,
                             struct.pack("<IIH", capture_id, offset, chunk)),
                "read GPIO capture",
            )
            if len(payload) != chunk * 4:
                raise RuntimeError("capture read returned the wrong number of bytes")
            values.extend(struct.unpack(f"<{chunk}I", payload))
            offset += chunk
        return CaptureResult(capture_id, captured, elapsed_us, tuple(values))

    def spi_config(self, baud, mode=0):
        response = self._ok(
            self.command(Command.SPI_CONFIG, struct.pack("<IB", baud, mode)),
            "configure SPI",
        )
        return struct.unpack("<I", response)[0]

    def spi_transfer(self, data, timeout=2000):
        data = bytes(data)
        if not data or len(data) > MAX_PAYLOAD - 1:
            raise ValueError("SPI transfer must contain 1..4095 bytes")
        response = self.command(Command.SPI_TRANSFER, data, timeout=timeout)
        return self._ok(response, "transfer SPI")


def wait_for_device(id_product, serial=None, timeout=10.0):
    """Poll until a 2e8a debug device with the requested PID (re-)enumerates."""
    import usb.core
    import usb.util

    deadline = time.monotonic() + timeout
    while True:
        for device in usb.core.find(find_all=True, idVendor=0x2E8A, idProduct=id_product):
            if serial is None or usb.util.get_string(device, device.iSerialNumber) == serial:
                return device
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.05)
