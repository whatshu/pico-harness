#!/usr/bin/env python3
"""Check chip family, flash bounds, SRAM budget and capture size of the Debug image."""
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
ARTIFACTS = ROOT / "RELEASE/artifacts"


def check(core, family, flash_size, cpu_arch, ram_limit, main_in_flash):
    stem = ARTIFACTS / f"pico_dualcore_debug_{core}"
    uf2 = Path(str(stem) + ".uf2").read_bytes()
    elf = Path(str(stem) + ".elf")
    assert uf2 and len(uf2) % 512 == 0, f"{core}: invalid UF2 length"
    pages = []
    for offset in range(0, len(uf2), 512):
        a, b, flags, address, size, number, count, block_family = struct.unpack_from("<8I", uf2, offset)
        assert (a, b) == (0x0a324655, 0x9e5d5157), f"{core}: invalid UF2 header"
        assert struct.unpack_from("<I", uf2, offset + 508)[0] == 0x0ab16f30
        assert size == 256 and flags & 0x2000
        if block_family == 0xe48bff57:
            # Pico SDK's RP2350-E10 workaround block is explicitly ignored.
            assert core == "pico2" and flags & 0x8000
            assert struct.unpack_from("<I", uf2, offset + 32 + size)[0] == 0x9957e304
            continue
        assert block_family == family, f"{core}: UF2 is for the wrong chip"
        assert 0x10000000 <= address < address + size <= 0x10000000 + flash_size
        pages.append((number, count, address))
    assert pages
    assert [p[0] for p in pages] == list(range(len(pages)))
    assert all(p[1] == len(pages) for p in pages)
    assert [p[2] for p in pages] == list(range(0x10000000, 0x10000000 + len(pages) * 256, 256))
    symbols = {}
    for line in subprocess.check_output(["arm-none-eabi-nm", "-S", "--defined-only", elf], text=True).splitlines():
        fields = line.split()
        if len(fields) in (3, 4):
            symbols[fields[-1]] = (int(fields[0], 16), int(fields[1], 16) if len(fields) == 4 else None)
    assert symbols["capture_buffer"][1] == 32768 * 4, f"{core}: capture buffer was reduced"
    end = symbols["__end__"][0]
    heap_limit = symbols["__HeapLimit"][0]
    assert 0x20000000 <= end < heap_limit <= ram_limit
    remaining = heap_limit - end
    assert remaining >= 8192, f"{core}: less than 8 KiB left for C heap (RTOS heap is already reserved)"
    assert (symbols["main"][0] < 0x20000000) == main_in_flash
    attributes = subprocess.check_output(["arm-none-eabi-readelf", "-A", elf], text=True)
    assert f"Tag_CPU_arch: {cpu_arch}\n" in attributes, f"{core}: incorrect CPU architecture"
    print(f"{core}: UF2 family 0x{family:08x}, 32768 samples, {remaining} bytes C heap headroom; OK")


if __name__ == "__main__":
    check("pico", 0xe48bff56, 2 * 1024 * 1024, "v6S-M", 0x20040000, True)
