/* SPDX-License-Identifier: MIT */
#ifndef PH_SUMP_H
#define PH_SUMP_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* Logic Analyzer (CDC1) SUMP/OLS protocol.
 *
 * The LA CDC carries the classic SUMP command set (as understood by
 * sigrok's `ols` driver): short commands (0x00 reset, 0x01 run, 0x02 ID,
 * 0x04 metadata) and long opcode+4-byte commands (0x80 divider, 0x81 read
 * count, 0x82 flags, 0xc0/0xc1/0xc2 stage-0 trigger mask/value/config).
 * Samples stream back as 32-bit words (bit i = SUMP channel i), oldest first.
 *
 * core0 parses commands and streams samples; core1 owns the capture engine
 * (la_*), so Run/Reset are passed over the core0/core1 request mailbox.
 */

void ph_sump_init(void);

/* core0: drain the LA CDC, parse SUMP commands, arm captures, stream the
 * completed sample buffer. Call from the USB pump loop. */
void ph_sump_pump(void);

/* core1: service the SUMP capture engine (start / poll / cancel). Call once
 * per worker iteration; this is the sole owner of capture completion polling. */
void ph_sump_worker(void);

void ph_sump_reset(void);

#endif
