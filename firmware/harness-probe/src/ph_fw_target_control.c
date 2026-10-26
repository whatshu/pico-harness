/* SPDX-License-Identifier: MIT */
/*
 * ============================================================================
 * Target control layer
 * ============================================================================
 *
 * The decision half of the target split; the execution half is target_io.c,
 * and both share target.h.  This file answers one question, repeatedly:
 *
 *   "Is the Work rail powered AND healthy enough for the IO layer to drive?"
 *
 * That single decision feeds ph_fw_target_io_set_ready(), the gate that keeps every
 * Work pin high-impedance while the rail is off.  All sensing (Work voltage,
 * enable/fault reads) and all actuation (enable/run/boot) reach the hardware
 * through the target_* API below; no controller registers are touched here.
 *
 * The policy:
 *   - ready ⇔ Work voltage ≥ 2.8 V, power-enable asserted, and no power fault;
 *   - re-evaluated every 5 ms from core 0's monitoring loop,
 *   - always armed with power OFF: before the rail drops we remove all
 *     cross-domain drive, and we never restore it without a fresh poll.
 *
 * Core safety note: the poll cadence and the ADC are shared with core 1's
 * worker loop (see ph_fw_target_work_mv()), which is why re-evaluation is throttled
 * and the ADC access is serialized inside the IO layer.
 */

#include "ph_fw_target.h"

static uint32_t last_poll_us;

void ph_fw_target_control_poll(void)
{
    uint32_t now = time_us_32();

    if ((uint32_t)(now - last_poll_us) < 5000)
        return;

    last_poll_us = now;
    bool ready = ph_fw_target_work_mv() >= 2800 && gpio_get(PH_FW_WORK_ENABLE_PIN) &&
                 gpio_get(PH_FW_WORK_FAULT_PIN);

    if (ready != ph_fw_target_io_ready())
        ph_fw_target_io_set_ready(ready);
}

void ph_fw_target_power_set(bool enabled)
{
    /* Remove all cross-domain drive BEFORE cutting the branch supply. */
    ph_fw_target_io_set_ready(false);
    gpio_put(PH_FW_WORK_ENABLE_PIN, enabled);

    if (enabled) {
        busy_wait_ms(20);
        last_poll_us = time_us_32() - 5000;
        ph_fw_target_control_poll();
    } else {
        gpio_put(PH_FW_WORK_RUN_PIN, false);
        gpio_put(PH_FW_WORK_BOOT_PIN, false);
    }
}
