use std::time::Duration;

use ph_app_link::{CmsisDap, DeviceSelector, PhBoardLink, Result};

fn main() -> Result<()> {
    let mut link = PhBoardLink::open()?;
    println!("PDBG serial: {:?}", link.serial);
    println!("PDBG info: {}", link.info()?);
    println!("PDBG status: {:?}", link.status()?);
    println!("PDBG measurement: {:?}", link.measure()?);

    // The SWD engine is tested only when the Work branch is powered. Restore
    // the default-off state even if the target does not answer.
    link.set_power(true)?;
    std::thread::sleep(Duration::from_millis(100));
    println!("PDBG powered status: {:?}", link.status()?);
    let swd_result = (|| -> Result<()> {
        let mut dap = CmsisDap::open_with_selector(DeviceSelector {
            serial: link.serial.clone(),
        })?;
        dap.connect_swd(1_000_000)?;
        println!("SWD packet size: {}", dap.packet_size());
        println!("SWD session connected");
        println!("SWD DP IDCODE: 0x{:08x}", dap.read_dp_idcode()?);
        println!("SWD CTRL/STAT: 0x{:08x}", dap.power_up_debug_port()?);
        dap.disconnect()
    })();
    let power_result = link.set_power(false);
    swd_result.and(power_result)
}
