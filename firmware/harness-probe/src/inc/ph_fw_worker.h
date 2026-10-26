/* SPDX-License-Identifier: MIT */
#ifndef PH_FW_WORKER_H
#define PH_FW_WORKER_H

void ph_fw_worker_init(void);
void ph_fw_worker_step(void);
void ph_fw_worker_thread(void);
void ph_fw_worker_reset(void);

/* Request the Debug core to drop to ROM BOOTSEL after flushing the pending
 * response (used by the ph DebugBootsel message). */
void ph_fw_request_self_bootsel(void);

#endif
