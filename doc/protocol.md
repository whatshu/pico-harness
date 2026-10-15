# Work console protocol v1

UART0 uses 8N1 on Work GP12/13, initially 115200 baud. SPI slave mode 0 uses
SCK=18, MISO=19, MOSI=20, CS=21. Debug is the master. Firmware does not change
these pins for logic sampling; the Debug sampler passively observes their pads.

UART frames are little endian:

| Bytes | Field |
| --- | --- |
| 0..1 | ASCII `PH` |
| 2 | Version = 1 |
| 3 | Kind |
| 4..5 | Sequence |
| 6..7 | Payload length, 0..128 |
| 8.. | Payload |
| Last 2 | CRC16-CCITT, polynomial 0x1021, initial 0xffff, over version through payload |

Requests use nonzero sequences. Replies echo the sequence and set kind bit 7.
Unsolicited Console messages use sequence 0. Unknown or corrupt frames are ignored.
After an invalid header or CRC the parser searches for the next `PH` frame.
Partial frames expire after an idle gap of four 8N1 characters or 20 ms,
whichever is longer; a complete valid frame already buffered after the damaged
prefix is retained. The parser uses a fixed 138-byte buffer.
Work also announces Hello capabilities with sequence 0 once per second when the
UART queue and hardware transmitter are idle, so a desktop can identify the
protocol after reconnecting to a silent application. An overdue announcement
waits for the next idle opportunity instead of accumulating behind application
traffic; ordinary Console frames also identify the protocol.

| Request | Payload | Reply |
| --- | --- | --- |
| 0x01 Hello | Empty | `01 07 00 08`: version, UART/SPI/baud capability bits, LE block size 2048 |
| 0x02 Console TX | 1..128 bytes | Empty ACK; application receives the bytes |
| 0x03 Baud prepare | Requested baud u32 | Requested baud u32, at the old rate |
| 0x04 Verify | Empty | Current actual baud u32 |
| 0x05 Commit baud | Empty | Current actual baud u32 |
| 0x06 Bulk TX prepare | Length u16, 1..2048 | Length u16, after SPI RX DMA is armed |
| 0x08 Bulk RX prepare | Empty | Available length u16, after SPI TX DMA is armed |

0x90 delivers an unsolicited short Console message. 0x91 announces a pending
SPI Console message (length u16). After each exact-length SPI transfer, Work
sends 0x87 with the prepare request's sequence and CRC16 of the transmitted
or received bytes. A zero-length RX reply means no pending message. SPI data
contains only payload; UART carries its boundaries and integrity confirmation.
The host must validate this confirmation before treating the transfer as complete.
Prepared transfers expire after 2 seconds.

Baud prepare schedules a switch at least 100 ms later and waits for the old-rate
ACK to leave the UART. Application writes and new bulk prepares are backpressured
until commit or rollback so they cannot prevent the old-rate queue from draining.
If it cannot drain within 1 second, prepare expires and Work stays at the old
rate. After switching, the commit lease in microseconds is
`max(1_500_000, ceil(640_000_000 / actual_baud) + 500_000)`: 64 bytes of 8N1
handshake traffic plus 0.5 seconds of margin. This is 2,633,334 µs at 300 baud
and remains 1.5 seconds at 640 baud or higher. Work restores the previous rate
when that lease expires without commit. The host changes its endpoint, verifies
actual baud within 2%, and commits.
On failure the host waits the new-rate commit lease plus at least 100 ms before
verifying the previous rate; if a lost commit ACK left Work at the new
rate, it reaches Work there and coordinates a switch back. Unverified restoration
is reported as an error rather than a successful rollback.
