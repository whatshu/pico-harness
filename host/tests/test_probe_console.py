"""Compile production Debug UART parsing/framing against deterministic IO adapters."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
FW = ROOT / 'firmware/harness-probe'


class ProbeConsoleChecks(unittest.TestCase):
    def test_raw_trace_preserves_heartbeat_invalid_rx_and_partial_tx(self):
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            sdk = '''#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#define clk_peri 0
uint32_t clock_get_hz(unsigned);
uint64_t time_us_64(void);
void busy_wait_until(uint64_t);
void tight_loop_contents(void);
bool ph_fw_target_io_ready(void);
bool ph_fw_target_serial_config(uint8_t,uint32_t,uint8_t,uint8_t,uint8_t,uint32_t*);
size_t ph_fw_target_serial_read(uint8_t,uint8_t*,size_t);
size_t ph_fw_target_serial_write(uint8_t,const uint8_t*,size_t);
'''
            (temp / 'sdk.h').write_text(sdk)
            for header in ('hardware/clocks.h', 'pico/stdlib.h'):
                path = temp / header
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('')
            binary = temp / 'console'
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-DPH_FW_TARGET_H', '-include', str(temp / 'sdk.h'),
                            '-I', str(temp), '-I', str(FW / 'src/inc'),
                            '-I', str(FW / 'src'), '-I', str(ROOT / 'third_party/nanopb'),
                            str(FW / 'src/ph_console.c'), str(FW / 'tests/console_regression.c'),
                            '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

    def test_trace_usb_backpressure_preserves_envelopes_and_control_priority(self):
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            for header in ('hardware/sync.h', 'pico/stdlib.h', 'FreeRTOS.h', 'task.h', 'tusb.h'):
                path = temp / header
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('#include "hardware_stub.h"\n')
            binary = temp / 'trace'
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-ffunction-sections', '-fdata-sections', '-Wl,--gc-sections',
                            '-DPICO_RP2040=1', '-DPICO_RP2350=0',
                            '-I', str(temp), '-I', str(FW / 'tests'),
                            '-I', str(FW / 'boards'), '-I', str(FW / 'src/inc'),
                            '-I', str(FW / 'src'), '-I', str(ROOT / 'third_party/nanopb'),
                            str(FW / 'tests/terminal_trace_regression.c'),
                            str(FW / 'src/ph.pb.c'),
                            *(str(ROOT / 'third_party/nanopb' / f'pb_{part}.c')
                              for part in ('common', 'encode', 'decode')),
                            '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)
