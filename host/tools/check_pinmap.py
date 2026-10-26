#!/usr/bin/env python3
"""Validate the dualcore firmware pin map against the current board netlist."""
from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONFIG = ROOT / "firmware/harness-probe/boards/pico_dualcore_config.h"
SELFTEST = ROOT / "firmware/test/work-selftest/main.c"
NETLIST = ROOT / "pcb/dualcore-netlist.enet"

EXPECTED = {
    "PROBE_PIN_SWCLK": 14,
    "PROBE_PIN_SWDIO": 15,
    "PROBE_PIN_RESET": 22,
    "PROBE_UART0_TX": 20,
    "PROBE_UART0_RX": 21,
    "PH_FW_WORK_ENABLE_PIN": 0,
    "PH_FW_WORK_FAULT_PIN": 1,
    "PH_FW_WORK_RUN_PIN": 22,
    "PH_FW_WORK_BOOT_PIN": 26,
    "PH_FW_WORK_ADC_PIN": 28,
    "PH_FW_INA_SDA_PIN": 4,
    "PH_FW_INA_SCL_PIN": 5,
    "PH_FW_INA_ALERT_PIN": 2,
    "PH_FW_SPI_RX_PIN": 8,
    "PH_FW_SPI_CS_PIN": 9,
    "PH_FW_SPI_SCK_PIN": 10,
    "PH_FW_SPI_TX_PIN": 11,
}


def macro_value(source: str, name: str) -> int:
    match = re.search(rf"^#define\s+{name}\s+(\d+)$", source, re.MULTILINE)
    if not match:
        raise AssertionError(f"missing numeric macro {name}")
    return int(match.group(1))


def check_netlist() -> None:
    data = json.loads(NETLIST.read_text())
    components = {
        component["props"]["Designator"]: component
        for component in data["components"].values()
    }
    debug_nets = {
        int(re.match(r"GP(\d+)(?:_|$)", pin["name"])[1]): pin["net"]
        for pin in components["U1"]["pinInfoMap"].values()
        if re.match(r"GP\d+(?:_|$)", pin["name"])
    }
    expected_nets = {
        14: "DBG_SWCLK", 15: "DBG_SWDIO", 22: "DBG_RUN_ASSERT",
        26: "DBG_BOOT_ASSERT", 0: "DBG_PWR_EN", 1: "DBG_PWR_FAULT_N",
        28: "DBG_WORK_3V3_ADC", 4: "DBG_I2C_SDA", 5: "DBG_I2C_SCL",
        2: "DBG_INA_ALERT_N", 20: "DBG_UART_TX", 21: "DBG_UART_RX",
        8: "DBG_SPI_RX", 9: "DBG_SPI_CSN", 10: "DBG_SPI_SCK", 11: "DBG_SPI_TX",
    }
    for pin, net in expected_nets.items():
        if debug_nets.get(pin) != net:
            raise AssertionError(f"Debug GP{pin}: expected {net}, got {debug_nets.get(pin)}")


def main() -> int:
    config = CONFIG.read_text()
    for name, expected in EXPECTED.items():
        actual = macro_value(config, name)
        if actual != expected:
            raise AssertionError(f"{name}: expected {expected}, got {actual}")
    if "#define PH_FW_GPIO_CAPTURE_MASK 0x1c3ffc20u" not in config:
        raise AssertionError("dualcore GPIO capture mask is missing")
    selftest = SELFTEST.read_text()
    for text in ("gpio_set_function(12, GPIO_FUNC_UART)",
                 "gpio_set_function(13, GPIO_FUNC_UART)",
                 "pico_harness_spi_init()"):
        if text not in selftest:
            raise AssertionError(f"Work self-test does not contain {text}")
    engine = (ROOT / "firmware/harness-target/src/spi_link.c").read_text()
    for text in ("pio_sm_set_consecutive_pindirs(pio0, sm, 19, 1, true)",
                 "sm_config_set_in_pins(&config, 20)",
                 "sm_config_set_out_pins(&config, 19, 1)"):
        if text not in engine:
            raise AssertionError(f"Shared Work SPI engine does not contain {text}")
    program = (ROOT / "firmware/harness-target/src/spi_slave.pio").read_text()
    for pin in (18, 21):
        if not re.search(rf"wait [01] gpio {pin}\b", program):
            raise AssertionError(f"Shared SPI program does not sample Work GP{pin}")
    if NETLIST.exists():
        check_netlist()
    else:
        print("pcb/dualcore-netlist.enet not present; skipping netlist comparison")
    print("dualcore pin map synchronized")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
