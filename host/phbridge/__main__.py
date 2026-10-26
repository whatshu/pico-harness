"""`ph` — PicoHarness command line (python, uses host/phbridge).

`python3 -m phbridge ...` — the bring-up and scripting interface that supersedes
the retired `host/pdbg` package. All device access runs on the phd host through socket IPC.
"""

from __future__ import annotations

import argparse
import os
import sys

from phbridge.client import PhClient, scan_terminals


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
    # Independent readers can inspect output retained before this process opened.
    c.cursor = 0
    event = c.wait_event("console_data", timeout=1.0)
    if event is not None:
        sys.stdout.write(event.console_data.data.decode("utf-8", "replace"))
        sys.stdout.flush()
    else:
        print("(no console output)")


def cmd_term(c: PhClient, args):
    r = c.term_config(args.baud)
    if r is not None and r.WhichOneof("body") == "term_config":
        print(f"actual={r.term_config.actual_baud}")
    else:
        sys.exit(f"term config failed: status={getattr(r, 'status', 'no reply')}")


def cmd_bulk(c: PhClient, args):
    from pathlib import Path
    result = c.bulk_write(Path(args.file).read_bytes())
    print(f"bulk done: ok={result['ok']} bytes={result['bytes']} crc=0x{result['crc32c']:08x}")


def cmd_flash(c: PhClient, args):
    import time
    upload = c.ipc.upload(args.uf2)
    task = c.ipc.call("task.start", {"kind": "debug_flash", "upload": upload, "serial": c.serial, "picotool": os.environ.get("PICOTOOL", "picotool")})["id"]
    while True:
        status = c.ipc.call("task.status", {"id": task})
        if status["state"] != "running":
            if status["state"] != "succeeded":
                sys.exit(status.get("error") or status["state"])
            print("Debug firmware updated")
            return
        time.sleep(0.1)


def main() -> None:
    p = argparse.ArgumentParser(prog="ph")
    p.add_argument("-p", "--port", default=None, metavar="TTY", help="probe serial or port on the phd host (auto if unambiguous)")
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
        print(f"Probe: {port or '(none)'}")
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