"""Online update of the stacked debug core over the PDBG link.

The debug core sits under the work core with no reachable BOOTSEL button, so
updates go through firmware command DEBUG_ENTER_BOOTSEL: the probe flushes its
response, reboots into the ROM USB bootloader, and picotool writes the
family-specific UF2.  The host validates the UF2 chip family, pins picotool to
the bootloader by USB bus/address, then reboots the probe into the app and
verifies it re-enumerated with the same serial number.

The ROM bootloader reports a different USB serial string than the application
firmware, so the load is targeted by bus/address rather than by --ser.
"""

import struct
import subprocess
from pathlib import Path

from .device import Probe, wait_for_device
from .protocol import Command

PROBE_USB = (0x2E8A, 0x000C)
UF2_FAMILIES = {"pico": 0xE48BFF56, "pico2": 0xE48BFF59}
BOOTSEL_USB = {"pico": (0x2E8A, 0x0003), "pico2": (0x2E8A, 0x000F)}
BOOTSEL_TO_CORE = {pid: core for core, (_, pid) in BOOTSEL_USB.items()}

UF2_MAGIC_START = (0x0A324655, 0x9E5D5157)
UF2_MAGIC_END = 0x0AB16F30
UF2_BLOCK = 512
UF2_FLAG_FAMILY_ID_PRESENT = 0x2000
UF2_RP2350_E10_FAMILY = 0xE48BFF57  # SDK workaround container, never a flash image


def is_dualcore_probe(info_string):
    return any(field == "dualcore" or field.startswith("dualcore-")
               for field in info_string.split(";"))


def probe_core(info_string):
    """Extract the debug core's chip name ("pico" or "pico2") from GET_INFO."""
    for field in info_string.split(";"):
        if field in UF2_FAMILIES:
            return field
    raise RuntimeError(f"probe does not report an updatable core: {info_string!r}")


def debug_image_for_probe(probe, artifacts):
    """Select the image for a running dualcore Debug probe."""
    info = Probe._ok(probe.command(Command.GET_INFO),
                     "get info").rstrip(b"\0").decode()
    if not is_dualcore_probe(info):
        raise RuntimeError(f"connected probe is not a dualcore Debug core: {info}")
    core = probe_core(info)
    if core != "pico":
        raise RuntimeError(f"target Debug core must be RP2040, got {core}")
    image = Path(artifacts) / f"pico_dualcore_debug_{core}.uf2"
    if not image.is_file():
        raise FileNotFoundError(f"{image} is missing; run ./build.sh build first")
    return image


def uf2_family(path):
    """Return the chip family ID of the flash blocks in a UF2 file."""
    with open(path, "rb") as source:
        while True:
            block = source.read(UF2_BLOCK)
            if not block:
                break
            if len(block) != UF2_BLOCK:
                raise ValueError(f"{path}: truncated UF2 block")
            words = struct.unpack_from("<8I", block)
            if (words[:2] != UF2_MAGIC_START or
                    struct.unpack_from("<I", block, 508)[0] != UF2_MAGIC_END):
                raise ValueError(f"{path}: not a UF2 file")
            flags, family = words[2], words[7]
            if family == UF2_RP2350_E10_FAMILY:
                continue
            if not flags & UF2_FLAG_FAMILY_ID_PRESENT:
                raise ValueError(f"{path}: family-less UF2 cannot be matched to a chip")
            return family
    raise ValueError(f"{path}: no UF2 flash block with a family ID found")


def _running_bootsel():
    """Return the RP-series devices currently exposing a ROM BOOTSEL PID."""
    import usb.core
    return [device for device in usb.core.find(find_all=True, idVendor=0x2E8A)
            if device.idProduct in BOOTSEL_TO_CORE]


def update_debug_core(probe, uf2_path, picotool="picotool",
                      bootsel_timeout=10.0, reboot_timeout=20.0, log=print):
    """Load a UF2 into the stacked debug core.

    probe may be None when the device is already in ROM BOOTSEL (a manual
    button boot), in which case the core family is inferred from the BOOTSEL
    product ID and the probe is never rebooted by this function.
    """
    if probe is not None:
        info = Probe._ok(probe.command(Command.GET_INFO),
                         "get info").rstrip(b"\0").decode()
        core = probe_core(info)
        serial = probe.serial
    else:
        serial = None
        candidates = _running_bootsel()
        if len(candidates) != 1:
            raise SystemExit(
                "no PDBG probe in application mode and "
                + ("multiple RP-series devices are in BOOTSEL; unplug the "
                   "others and retry"
                   if candidates else
                   "no RP-series device in ROM BOOTSEL on the bus"))
        bootsel = candidates[0]
        core = BOOTSEL_TO_CORE[bootsel.idProduct]

    # Validate the image before rebooting anything: a family mismatch must
    # leave the probe untouched and no bootloader entry requested.
    family = UF2_FAMILIES[core]
    actual = uf2_family(uf2_path)
    if actual != family:
        raise SystemExit(
            f"{uf2_path}: UF2 family 0x{actual:08x} targets the wrong chip; "
            f"the {core} debug core needs 0x{family:08x}. "
            "Nothing was flashed."
        )

    if probe is not None:
        log(f"debug core is {core} (serial {serial}); entering ROM BOOTSEL")
        Probe._ok(probe.command(Command.DEBUG_ENTER_BOOTSEL, b"BOOT"),
                  "enter debug-core update mode")
        probe.close()
        bootsel = wait_for_device(BOOTSEL_USB[core][1], timeout=bootsel_timeout)
        if bootsel is None:
            raise SystemExit(
                f"debug core did not enter ROM BOOTSEL within "
                f"{bootsel_timeout:.0f} s.  Check that no other RP-series "
                "device shadows it and retry."
            )
    else:
        log(f"device is already in ROM BOOTSEL as a {core}; loading the UF2")

    try:
        # Bus/address pin picotool to exactly this debug core, never the Work
        # core or any other RP-series device.  Load leaves the device in
        # BOOTSEL, so reboot it into the application afterwards.
        subprocess.run([picotool, "load", "-v",
                        "--bus", str(bootsel.bus),
                        "--address", str(bootsel.address),
                        "--family", f"0x{family:08x}", str(uf2_path)],
                       check=True)
        subprocess.run([picotool, "reboot",
                        "--bus", str(bootsel.bus),
                        "--address", str(bootsel.address)], check=True)
    except FileNotFoundError as error:
        raise SystemExit(
            f"BOOTSEL is ready but {picotool!r} was not found; run "
            f"sudo {picotool} load -v {uf2_path}"
        ) from error
    except PermissionError as error:
        raise SystemExit(
            f"BOOTSEL is ready but USB permission denied ({error}). "
            f"Run: sudo {picotool} load -v {uf2_path}"
        ) from error
    if wait_for_device(PROBE_USB[1], serial=serial, timeout=reboot_timeout) is None:
        raise SystemExit(
            f"updated probe did not re-enumerate within {reboot_timeout:.0f} s"
        )
    updated = Probe.open(serial)
    version = Probe._ok(updated.command(Command.GET_INFO),
                        "get info").rstrip(b"\0").decode()
    log(f"update complete: {version}")
