"""Exercise the actual Python socket client against a fragmented RPC peer."""
import json
import socket
import threading
import unittest

from phbridge.ipc import Client, PhdError


class Peer:
    def __init__(self, respond):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.address = "%s:%s" % self.listener.getsockname()
        self.calls = []
        self.error = None
        self.respond = respond
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def run(self):
        try:
            connection, _ = self.listener.accept()
            connection.settimeout(5)
            with connection, connection.makefile("rb") as stream:
                while frame := stream.readline():
                    request = json.loads(frame)
                    self.calls.append(request)
                    if request["method"] == "hello":
                        reply = {"result": {"version": 1, "instance": "fixture"}}
                    else:
                        reply = self.respond(request)
                    if reply is None:
                        return
                    reply = {"jsonrpc": "2.0", "id": request["id"], **reply}
                    frame = json.dumps(reply).encode() + b"\n"
                    for part in range(0, len(frame), 3):
                        connection.sendall(frame[part:part + 3])
        except BaseException as error:
            self.error = error
        finally:
            self.listener.close()

    def close(self):
        self.thread.join(6)
        if self.thread.is_alive():
            raise AssertionError("socket peer did not exit")
        if self.error:
            raise self.error


class IpcTests(unittest.TestCase):
    def connect(self, respond):
        peer = Peer(respond)
        self.addCleanup(peer.close)
        client = Client(peer.address, "fixture-token")
        self.addCleanup(client.close)
        return peer, client

    def test_fragmentation_null_result_and_request_ids(self):
        peer, client = self.connect(lambda _: {"result": None})
        self.assertIsNone(client.call("board.power", {"enabled": True}))
        self.assertEqual(client.instance, "fixture")
        self.assertEqual([c["id"] for c in peer.calls], [1, 2])
        self.assertEqual(peer.calls[0]["params"]["token"], "fixture-token")

    def test_structured_error_and_connection_remains_usable(self):
        _, client = self.connect(lambda _: {"error": {
            "code": -32003, "kind": "busy", "message": "exclusive lease", "retryable": True}})
        for _ in range(2):
            with self.assertRaises(PhdError) as caught:
                client.call("board.power")
            self.assertEqual(caught.exception.kind, "busy")
            self.assertTrue(caught.exception.retryable)

    def test_lost_mutation_reply_is_never_replayed(self):
        peer, client = self.connect(lambda _: None)
        with self.assertRaises(ConnectionError):
            client.call("board.reset", {"milliseconds": 10})
        self.assertEqual([c["method"] for c in peer.calls], ["hello", "board.reset"])

    def test_rejects_response_with_both_result_and_error(self):
        _, client = self.connect(lambda _: {"result": None, "error": {}})
        with self.assertRaises(ConnectionError):
            client.call("daemon.health")

    def test_rejects_wrong_request_id(self):
        _, client = self.connect(lambda _: {"id": 999, "result": {}})
        with self.assertRaises(ConnectionError):
            client.call("daemon.health")
