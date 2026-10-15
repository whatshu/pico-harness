# dualcore Debug Core firmware

This firmware targets the two Debug Core probe variants of the optional carrier board:
`pico_dualcore_pico` for RP2040 Pico and `pico_dualcore` for RP2350
Pico 2. All board-specific definitions live in `firmware/harness-probe/boards/`; the generic
`firmware/harness-probe/include/` directory contains only CMSIS-DAP and shared interfaces.

The firmware exposes CMSIS-DAP v2 and the vendor PDBG Link bulk interface. The
PDBG Link carries the dualcore UART, SPI, power, INA237 and logic-analyzer commands.
Pin mapping is defined per board in `firmware/harness-probe/boards/`.

Build from the repository root with the installed SDK:

```sh
export PICO_SDK_PATH=/path/to/pico-sdk
./build.sh build-debug
```

The SDK, picotool, OpenOCD and ARM compiler are host tools. They are not
vendored in this repository.
