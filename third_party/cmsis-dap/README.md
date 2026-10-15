# CMSIS-DAP

This directory contains the complete CMSIS-DAP source tree used by the
harness-probe firmware. It is based on the CMSIS-DAP firmware sources imported
with Raspberry Pi Debug Probe v2.3.1 (`39e3c8d15ba36cd607a276d7dfe23bde03e88be0`).

The project-specific `firmware/harness-probe/include/DAP_config.h` remains the
target adapter configuration. The files in this directory are otherwise kept
as an upstream dependency and are built through the firmware CMake target.

CMSIS-DAP source files retain their upstream licensing notices.
