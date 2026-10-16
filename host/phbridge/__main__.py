"""`ph` — PicoHarness command line (python, uses host/phbridge).

`python3 -m phbridge ...` — the bring-up and scripting interface that supersedes
the retired `host/pdbg` package. The Rust `ph` bin uses the same PH protocol.
"""

from __future__ import annotations

import argparse
import sys

import phbridge
import phbridge.ph_pb2 as pb
from phbridge.client import PhClient, scan_terminals


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 & -(crc & 1))
    return (~crc) & 0xFFFFFFFF


def open_client(port: str | None, require: bool = True) -> PhClient:
    port = port or scan_terminals()
    if not port:
        if require:
            sys.exit("no PH Terminal found (use a tty path as an argument)")
        return None
    return PhClient(port)


def cmd_power(c: PhClient, args):
    r = c.power_set(args.state == "on")
    if r is None or r.status != 0:
        sys.exit(f"power {args.state} failed: status={getattr(r, 'status', 'no reply')}")


def cmd_measure(c: PhClient, _):
    r = c.measure()
    if r is None or r.WhichOneof("body") != "measure":
        sys.exit("measure failed")
    m = r.measure
    print(f"shunt={m.shunt_uv}uV vbus={m.vbus_uv}uV current={m.current_ua}uA power={m.power_uw}uW")


def cmd_status(c: PhClient, _):
    r = c.get_status()
    if r is None or r.WhichOneof("body") != "get_status":
        sys.exit("status failed")
    s = r.get_status
    print(
        f"power_enabled={s.power_enabled} power_good={s.power_good} "
        f"ina_alert={s.ina_alert} work_fault={s.work_fault} vbus_mv={s.vbus_mv}"
    )


def cmd_console_send(c: PhClient, args):
    r = c.console_tx(args.text.encode())
    if r is not None and r.status not in (0, 7):
        sys.exit(f"console send: status={r.status}")


def cmd_console_read(c: PhClient, _):
    import time

    deadline = time.time() + 1.0
    printed = 0
    while time.time() < deadline and printed < 64:
        events = c.drain_events()
        for ev in events:
            if ev.WhichOneof("body") == "console_data":
                sys.stdout.write(ev.console_data.data.decode("utf-8", "replace"))
                sys.stdout.flush()
                printed += 1
        if printed == 0 and not c.s.in_waiting:
            # pull a short read window for unsolicited output
            ev = c.wait_event("console_data", timeout=0.2)
            if ev is not None:
                sys.stdout.write(ev.console_data.data.decode("utf-8", "replace"))
                sys.stdout.flush()
                printed += 1
    if printed == 0:
        print("(no console output)")


def cmd_term(c: PhClient, args):
    r = c.term_config(args.baud)
    if r is not None and r.WhichOneof("body") == "term_config":
        print(f"actual={r.term_config.actual_baud}")
    else:
        sys.exit(f"term config failed: status={getattr(r, 'status', 'no reply')}")


def cmd_bulk(c: PhClient, args):
    import usb.core
    import usb.util

    payload = open(args.file, "rb").read()
    crc = crc32c(payload)
    r = c.bulk_tx_begin(route=1, length=len(payload), crc=crc)  # WORK_SPI
    if r is None or r.status != 0:
        sys.exit(f"bulk begin failed: status={getattr(r, 'status', 'no reply')}")

    dev = usb.core.find(idVendor=0x2E8A, idProduct=0x000C)
    if dev is None:
        sys.exit("probe not found over USB")
    itf = None
    for i in dev.get_active_configuration():
        if i.bInterfaceNumber == 3:
            itf = i
    usb.util.claim_interface(dev, 3)
    try:
        out = usb.util.find_descriptor(
            itf,
            custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
            == usb.util.ENDPOINT_OUT,
        )
        out.write(payload, timeout=10000)
        done = c.wait_event("bulk_tx_done", timeout=8.0)
        if done is None:
            sys.exit("no BulkTxDone within 8s")
        print(
            f"bulk done: ok={done.bulk_tx_done.ok} "
            f"crc_recv=0x{done.bulk_tx_done.crc_recv:08x} expected=0x{crc:08x}"
        )
        if not done.bulk_tx_done.ok:
            sys.exit(1)
    finally:
        usb.util.release_interface(dev, 3)


def cmd_flash(c: PhClient, args):
    import shutil
    import subprocess
    import time

    picotool = __import__("os").environ.get("PICOTOOL") or shutil.which("picotool")
    if not picotool:
        sys.exit("picotool not found")
    r = c.debug_bootsel()
    if r is None or r.status != 0:
        sys.exit("debug_bootsel failed")
    time.sleep(2)
    rc = subprocess.run([picotool, "load", "-x", args.uf2])
    if rc.returncode != 0:
        sys.exit(f"picotool load failed ({rc.returncode})")


def main() -> None:
    p = argparse.ArgumentParser(prog="ph")
    p.add_argument("-p", "--port", default=None, metavar="TTY", help="PH Terminal tty (auto)")
    sub = p.add_subparsers(dest="command", required=True)

    sub.add_parser("device", help="list devices").add_argument("list", nargs="?", default="list")
    sp = sub.add_parser("power")
    sp.add_argument("state", choices=["on", "off"])
    sub.add_parser("measure")
    sub.add_parser("status")
    spc = sub.add_parser("console")
    spc_args = spc.add_subparsers(dest="sub", required=True)
    spc_send = spc_args.add_parser("send")
    spc_send.add_argument("text")
    spc_args.add_parser("read")
    spc_term = sub.add_parser("term")
    spc_term.add_argument("baud", type=int)
    spc_bulk = sub.add_parser("bulk")
    spc_bulk.add_argument("file")
    spc_flash = sub.add_parser("flash")
    spc_flash.add_argument("uf2")

    args = p.parse_args()

    if args.command == "device":
        port = scan_terminals()
        print(f"PH Terminal: {port or '(none)'}")
        return

    c = open_client(args.port)
    try:
        if args.command == "power":
            cmd_power(c, args)
        elif args.command == "measure":
            cmd_measure(c, args)
        elif args.command == "status":
            cmd_status(c, args)
        elif args.command == "console":
            if args.sub == "send":
                cmd_console_send(c, args)
            else:
                cmd_console_read(c, args)
        elif args.command == "term":
            cmd_term(c, args)
        elif args.command == "bulk":
            cmd_bulk(c, args)
        elif args.command == "flash":
            cmd_flash(c, args)
    finally:
        c.close()


if __name__ == "__main__":
    main()