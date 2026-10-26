//! Hardware-gated checks: run with `cargo test -p ph-device -- --ignored`.
//! Requires a connected PicoHarness probe in application mode.

use ph_device::PhBoardLink;

#[test]
#[ignore = "requires connected hardware"]
fn link_reports_identity_and_power_over_ph() {
    let mut link = PhBoardLink::open().expect("open PH control link");
    let info = link.info().expect("GetInfo");
    assert!(info.starts_with("PicoHarness/"), "identity: {info}");

    let status = link.status().expect("GetStatus");
    // Work rail is off by default; the poll must still answer.
    let _ = status.power_on;

    link.set_power(true).expect("PowerSet on");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let m = link.measure().expect("Measure");
    assert!(m.bus_mv > 0.0, "bus should rise with the rail on: {m:?}");
    link.set_power(false).expect("PowerSet off");
}

#[test]
#[ignore = "requires connected hardware"]
fn moved_channels_are_rejected_clearly() {
    let mut link = PhBoardLink::open().expect("open PH control link");
    let err = link.spi_transfer(&[0u8; 8]).unwrap_err();
    assert!(err.to_string().contains("Bulk"), "unexpected error: {err}");
}
