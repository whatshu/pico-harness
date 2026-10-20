"""Authenticated socket IPC shared by host scripts; no device drivers required.

Calls are never replayed after a transport error because a device may have
acted before the response was lost.
"""
from __future__ import annotations
import json
import os
from pathlib import Path
import socket
import sys

MAX_FRAME = 2 * 1024 * 1024


class PhdError(RuntimeError):
    def __init__(self, error: dict):
        self.code = error["code"]
        self.kind = error["kind"]
        self.retryable = error.get("retryable", False)
        super().__init__(f"{self.kind}: {error['message']}")


def state_dir() -> Path:
    if value := os.environ.get("PHD_STATE_DIR"):
        return Path(value)
    if sys.platform == "win32":
        return Path(os.environ["LOCALAPPDATA"]) / "PicoHarness" / "phd" / "data"
    if sys.platform == "darwin":
        return Path.home() / "Library/Application Support/org.PicoHarness.phd"
    return Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "phd"


class Client:
    def __init__(self, address: str | None = None, token: str | None = None, timeout: float = 30):
        address = address or os.environ.get("PHD_ADDR", "127.0.0.1:44840")
        if token is None:
            token = os.environ.get("PHD_TOKEN")
        if token is None:
            token = Path(os.environ.get("PHD_TOKEN_FILE", str(state_dir() / "token"))).read_text().strip()
        host, separator, port = address.rpartition(":")
        if not separator or not host:
            raise ValueError("PHD_ADDR must be host:port (IPv6: [address]:port)")
        self.socket = socket.create_connection((host.strip("[]"), int(port)), timeout=timeout)
        self.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.reader = self.socket.makefile("rb")
        self.sequence = 0
        self.instance = ""
        try:
            hello = self.call("hello", {"version": 1, "token": token})
            if hello["version"] != 1:
                raise RuntimeError("incompatible phd protocol version")
            self.instance = hello["instance"]
        except BaseException:
            self.close()
            raise

    def call(self, method: str, params: dict | None = None):
        self.sequence += 1
        frame = json.dumps({"jsonrpc": "2.0", "id": self.sequence, "method": method,
                            "params": params or {}}, separators=(",", ":")).encode() + b"\n"
        if len(frame) > MAX_FRAME:
            raise ValueError("phd request exceeds socket frame limit")
        try:
            self.socket.sendall(frame)
            frame = self.reader.readline(MAX_FRAME + 1)
            if not frame or not frame.endswith(b"\n") or len(frame) > MAX_FRAME:
                raise ConnectionError("phd closed the session or sent an invalid frame")
            reply = json.loads(frame)
            if (not isinstance(reply, dict) or reply.get("jsonrpc") != "2.0" or reply.get("id") != self.sequence
                    or ("result" in reply) == ("error" in reply)):
                raise ConnectionError("invalid phd response envelope")
        except (OSError, ValueError):
            self.close()
            raise
        if "error" in reply:
            raise PhdError(reply["error"])
        return reply["result"]

    def upload(self, path: str | Path) -> str:
        import base64
        path = Path(path)
        with path.open("rb") as source:
            size = os.fstat(source.fileno()).st_size
            upload = self.call("upload.begin", {"length": size, "extension": path.suffix.lstrip(".")})["id"]
            offset = 0
            while chunk := source.read(512 * 1024):
                self.call("upload.write", {"id": upload, "offset": offset,
                                          "data": base64.b64encode(chunk).decode("ascii")})
                offset += len(chunk)
            self.call("upload.finish", {"id": upload})
            return upload

    def close(self):
        self.reader.close()
        self.socket.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
