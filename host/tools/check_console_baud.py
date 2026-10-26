#!/usr/bin/env python3
"""Hardware regression: Apply each desktop preset, verify a fresh reply and raw trace.

Does not flash firmware or change Work power. Restores the initial UART rate.
Run from the repository with PYTHONPATH=host python3 host/tools/check_console_baud.py
--serial SERIAL [--require-trace] [--output FILE].
"""
import argparse
import base64
import binascii
import json
import time
from phbridge.ipc import Client

PRESETS = (9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--require-trace', action='store_true')
    parser.add_argument('--output')
    args = parser.parse_args()
    evidence = {'serial': args.serial, 'checks': [], 'trace': []}
    with Client() as client:
        cursor = client.call('device.open', {'serial': args.serial})['cursor']
        original = client.call('console.status')[1]
        evidence['initial_baud'] = original

        def write(data):
            client.call('console.write', {'data': base64.b64encode(data).decode()})

        def receive_until(token, timeout=5):
            nonlocal cursor
            output = b''
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                batch = client.call('events.read', {'cursor': cursor, 'limit': 128, 'wait_ms': 50})
                cursor = batch['cursor']
                assert not batch['gap'], 'daemon event history gap'
                for event in batch['events']:
                    if 'console' in event:
                        output += base64.b64decode(event['console'])
                    if 'uart_trace' in event:
                        evidence['trace'].append(event['uart_trace'])
                if token is not None and token in output:
                    return output
            if token is None:
                return output
            raise AssertionError(f'no fresh console response for {token!r}; received {output!r}')

        try:
            for busy in (False, True):
                for rate in PRESETS:
                    if busy:
                        write(b'help\n')
                    started = time.monotonic()
                    actual = client.call('console.configure', {'baud': rate})
                    elapsed = time.monotonic() - started
                    assert abs(actual-rate) <= rate * .02, (rate, actual)
                    # Unknown command returns a unique token; stale prompts cannot pass.
                    marker = f'baud-check-{time.monotonic_ns()}'.encode()
                    write(marker+b'\n')
                    response = receive_until(b"unknown command '"+marker+b"'")
                    status = client.call('console.status')
                    assert status[1] == actual, (status, actual)
                    result = dict(requested=rate, actual=actual, busy=busy,
                                  apply_seconds=elapsed, response=response.decode(errors='replace'),
                                  overruns=status[2])
                    evidence['checks'].append(result)
                    print(json.dumps({k: v for k, v in result.items() if k != 'response'}), flush=True)
            receive_until(None, 1.2)  # Include an idle Hello heartbeat.
        finally:
            client.call('console.configure', {'baud': original})
            evidence['restored_baud'] = client.call('console.status')[1]
            if args.output:
                from pathlib import Path
                Path(args.output).write_text(json.dumps(evidence, indent=2))
        if args.require_trace:
            assert evidence['trace'], 'no raw UART trace: probe firmware needs ut1'
            assert not any(t['dropped'] for t in evidence['trace']), 'firmware trace omitted bytes'
            buffers = {True: bytearray(), False: bytearray()}
            frames = set()
            for trace in evidence['trace']:
                pending = buffers[trace['tx']]
                pending.extend(base64.b64decode(trace['data']))
                while len(pending) >= 8:
                    length = int.from_bytes(pending[6:8], 'little')
                    if pending[:3] != b'PH\x01' or length > 128:
                        del pending[0]
                        continue
                    if len(pending) < length + 10:
                        break
                    frame = pending[:length + 10]
                    crc = int.from_bytes(frame[-2:], 'little')
                    if binascii.crc_hqx(frame[2:-2], 0xffff) == crc:
                        frames.add((trace['tx'], frame[3]))
                        del pending[:len(frame)]
                    else:
                        del pending[0]
            required = {(True, 3), (False, 0x83), (True, 4), (False, 0x84),
                        (True, 5), (False, 0x85), (False, 0x81)}
            assert required <= frames, f'missing raw heartbeat/handshake frames: {required-frames}'
            print('CRC-verified raw heartbeat and prepare/verify/commit in both directions')
    print(f"PASS: {len(evidence['checks'])} baud transitions with fresh responses; "
          f"{len(evidence['trace'])} raw trace records; initial baud restored")


if __name__ == '__main__':
    main()
