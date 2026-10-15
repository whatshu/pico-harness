# Third-party notices

PicoHarness's original code is MIT licensed. Existing notices take precedence
for imported or derived files.

| Component | License / source |
| --- | --- |
| Raspberry Pi Debug Probe | MIT; `firmware/harness-probe/LICENSE` and `UPSTREAM.md` |
| gusmanb LogicAnalyzer sampler | GPL-3.0-only; `firmware/harness-probe/LOGIC_ANALYZER.md` |
| CMSIS-DAP | Apache-2.0; `third_party/cmsis-dap/LICENSE` |
| FreeRTOS Kernel | MIT; `third_party/FreeRTOS-Kernel/LICENSE.md` |
| KiCad schematic API | See `third_party/kicad-sch-api/LICENSE` |
| Pico SDK, TinyUSB | Their upstream licenses; supplied by the configured Pico SDK |
| Rust dependencies | Each package's license; versions pinned in `desktop/Cargo.lock` |
| Noto CJK font subset | SIL Open Font License 1.1; `desktop/app/assets/FONT-LICENSE.txt` |

The Debug Core firmware combines the GPL sampler with other components and
must be distributed under GPL-3.0-only with corresponding source and build
instructions. The root MIT license does not change that obligation. The desktop
application and Work Core library do not incorporate that sampler.

Vendor datasheets and reference designs used while designing retain their authors' rights and are not redistributed with this repository.
