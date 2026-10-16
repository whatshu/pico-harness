/* SPDX-License-Identifier: MIT */
#ifndef PDBG_WORKER_H
#define PDBG_WORKER_H

void worker_init(void);
void worker_step(void);
void worker_thread(void);
void worker_reset(void);

/* Request the Debug core to drop to ROM BOOTSEL after flushing the pending
 * response (used by the ph DebugBootsel message). */
void ph_request_self_bootsel(void);

#endif
