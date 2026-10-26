"""Python and CLI interoperability with a real daemon, without USB hardware."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest

from phbridge.ipc import Client, PhdError

ROOT = Path(__file__).resolve().parents[2]
SUFFIX = ".exe" if os.name == "nt" else ""
PHD = ROOT / "ph-bridge/target/debug" / ("phd" + SUFFIX)
CLI = ROOT / "ph-bridge/target/debug" / ("phdcli" + SUFFIX)


@unittest.skipUnless(PHD.is_file() and CLI.is_file(), "build phd and phdcli first")
class DaemonTests(unittest.TestCase):
    def test_python_cli_authentication_upload_and_restart(self):
        with tempfile.TemporaryDirectory() as directory:
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                address = "127.0.0.1:%s" % reservation.getsockname()[1]
            token_file = Path(directory) / "token"
            environment = {**os.environ, "PHD_ADDR": address, "PHD_TOKEN_FILE": str(token_file), "PHD_STATE_DIR": directory}
            environment.pop("PHD_TOKEN", None)
            instances = []
            for run in range(2):
                with subprocess.Popen([str(PHD), "--listen", address, "--token-file", str(token_file)],
                                      env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE) as daemon:
                    try:
                        deadline = time.monotonic() + 10
                        while True:
                            try:
                                token = token_file.read_text().strip()
                                client = Client(address, token, timeout=2)
                                break
                            except (OSError, ValueError):
                                if daemon.poll() is not None or time.monotonic() >= deadline:
                                    self.fail("daemon failed to become ready")
                                time.sleep(0.02)
                        with client:
                            instances.append(client.instance)
                            status = subprocess.run([str(CLI), "--json", "daemon", "status"],
                                                    env=environment, capture_output=True, text=True, timeout=10, check=True)
                            self.assertEqual(json.loads(status.stdout)["instance"], client.instance)
                            with self.assertRaises(PhdError) as rejected:
                                Client(address, "wrong-token")
                            self.assertEqual(rejected.exception.kind, "unauthorized")
                            image = Path(directory) / "image.bin"
                            image.write_bytes(bytes(range(256)) * 4096)
                            upload = client.upload(image)
                            self.assertTrue(upload)
                            if run == 1 and os.name != "nt":
                                # Native Unix service managers stop with SIGTERM.
                                # It must take the normal shutdown path, not die
                                # before cleaning up sessions and subprocesses.
                                daemon.terminate()
                            else:
                                self.assertIsNone(client.call("daemon.stop"))
                        self.assertEqual(daemon.wait(timeout=10), 0)
                    finally:
                        if daemon.poll() is None:
                            daemon.terminate()
                            daemon.wait(timeout=10)
            self.assertNotEqual(*instances)

    def test_background_cli_start_is_idempotent_and_stop_releases_socket(self):
        with tempfile.TemporaryDirectory() as directory:
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                address = "127.0.0.1:%s" % reservation.getsockname()[1]
            environment = {**os.environ, "PHD_ADDR": address, "PHD_STATE_DIR": directory,
                           "PHD_TOKEN_FILE": str(Path(directory) / "token")}
            environment.pop("PHD_TOKEN", None)
            def cli(*arguments):
                result = subprocess.run([str(CLI), "--json", *arguments], env=environment,
                                        capture_output=True, text=True, timeout=15, check=True)
                return json.loads(result.stdout)
            try:
                first = cli("daemon", "start")
                second = cli("daemon", "start")
                self.assertEqual(first["instance"], second["instance"])
                self.assertTrue((Path(directory) / "phd.log").is_file())
            finally:
                if (Path(directory) / "token").exists():
                    cli("daemon", "stop")
            deadline = time.monotonic() + 5
            while True:
                try:
                    with socket.create_connection(("127.0.0.1", int(address.rsplit(":", 1)[1])), timeout=0.2):
                        pass
                except OSError:
                    break
                self.assertLess(time.monotonic(), deadline, "daemon retained its listener")
                time.sleep(0.02)
