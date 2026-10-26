"""Compatibility PH client backed by phd IPC; never opens USB or CDC.

Generated protobuf messages remain available to host scripts. phd owns firmware
sequencing and complete Bulk transactions.
"""
from __future__ import annotations
import base64
import time
from . import ph_pb2 as pb
from .ipc import Client


class PhClient:
    def __init__(self, port: str | None = None, timeout: float = 30):
        self.ipc = Client(timeout=timeout)
        self.events: list[pb.PhMessage] = []
        self.seq = 0
        try:
            params = {"port": port} if port and (port.startswith("/") or port.upper().startswith("COM")) else {"serial": port}
            opened = self.ipc.call("device.open", params)
            self.serial = opened["serial"]
            self.cursor = opened["cursor"]
        except BaseException:
            self.close()
            raise

    def close(self) -> None:
        self.ipc.close()

    def request(self, build, request_timeout: float = 2.0) -> pb.PhMessage:
        self.seq = self.seq % 0xFFFFFFFF + 1
        message = pb.PhMessage(seq=self.seq)
        build(message)
        previous = self.ipc.socket.gettimeout()
        try:
            self.ipc.socket.settimeout(request_timeout)
            response = self.ipc.call("ph.request", {"data": base64.b64encode(message.SerializeToString()).decode("ascii")})
        finally:
            if self.ipc.socket.fileno() >= 0:
                self.ipc.socket.settimeout(previous)
        return pb.PhMessage.FromString(base64.b64decode(response["data"], validate=True))

    def _poll(self, wait_ms: int = 0):
        batch = self.ipc.call("events.read", {"cursor": self.cursor, "limit": 128, "wait_ms": wait_ms})
        self.cursor = batch["cursor"]
        if batch["instance"] != self.ipc.instance or batch["gap"]:
            raise RuntimeError("phd event history changed or overflowed; console data was lost")
        for event in batch["events"]:
            if "ph" in event:
                self.events.append(pb.PhMessage.FromString(base64.b64decode(event["ph"], validate=True)))
        if len(self.events) > 1024:
            self.events.clear()
            raise RuntimeError("Python event buffer overflowed; queued device events were lost")

    def wait_event(self, kind: str, timeout: float = 2.0) -> pb.PhMessage | None:
        deadline = time.monotonic() + timeout
        while True:
            for index, event in enumerate(self.events):
                if event.WhichOneof("body") == kind:
                    return self.events.pop(index)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            self._poll(min(1000, max(1, int(remaining * 1000))))

    def drain_events(self) -> list[pb.PhMessage]:
        self._poll()
        events, self.events = self.events, []
        return events

    def get_info(self):
        return self.request(lambda m: m.get_info.SetInParent())

    def get_status(self):
        return self.request(lambda m: m.get_status.SetInParent())

    def power_set(self, enabled: bool):
        return self.request(lambda m: setattr(m.power_set, "enabled", enabled))

    def measure(self):
        return self.request(lambda m: m.measure.SetInParent())

    def console_tx(self, data: bytes):
        return self.request(lambda m: setattr(m.console_tx, "data", data))

    def term_config(self, baud: int):
        def build(message):
            message.term_config.baud = baud
            message.term_config.data_bits = 8
            message.term_config.stop_bits = 1
        return self.request(build)

    def bulk_write(self, data: bytes, route: str = "work_spi") -> dict:
        result = self.ipc.call("bulk.write", {"data": base64.b64encode(data).decode("ascii"), "route": route})
        event = pb.PhMessage()
        event.bulk_tx_done.ok = result["ok"]
        event.bulk_tx_done.crc_recv = result["crc32c"]
        self.events.append(event)
        return result

    def debug_bootsel(self):
        return self.request(lambda m: m.debug_bootsel.SetInParent())


def scan_terminals():
    """Select an unambiguous device on the daemon host without probing ports."""
    with Client() as client:
        devices = client.call("device.list")
    if len(devices) > 1:
        raise ValueError("multiple probes found; specify a serial number with -p")
    return devices[0]["serial"] if devices else None
