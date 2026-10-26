//! Headless semantic tests of the actual eframe application.
//!
//! AccessKit actions identify widgets by accessible names. No display server,
//! screenshot matching, screen coordinates, or operating-system input is used.
use super::*;
use egui_kittest::{
    kittest::{by, Queryable},
    Harness,
};

struct Gui {
    app: Harness<'static, App>,
    events: mpsc::Sender<live::Event>,
    commands: mpsc::Receiver<live::Command>,
}

impl Gui {
    fn new(page: Page, size: [f32; 2], language: usize) -> Self {
        let (session, events, commands) = live::Session::test_source();
        let app = Harness::builder().with_size(size).build_eframe(move |cc| {
            let mut app = App::new(cc);
            app.locale.selected = language;
            let mut device = Device::base("gui-fixture".into(), "PicoHarness test probe".into());
            device.page = page;
            device.online = true;
            device.power = true;
            device.live_ready = true;
            device.live_session = Some(session);
            app.devices.push(device);
            app
        });
        Self {
            app,
            events,
            commands,
        }
    }

    fn settle(&mut self) {
        // Live intentionally requests continuous repaint. Advance a bounded
        // number of frames rather than waiting for an idle application.
        self.app.run_steps(3);
    }

    fn click(&mut self, label: &str) {
        self.app.get_by_label(label).click();
        self.settle();
    }
}

#[test]
fn page_shortcuts_and_tab_clicks_select_the_same_pages() {
    let mut gui = Gui::new(Page::Overview, [960., 700.], 0);
    for (index, (page, key)) in Page::ALL
        .into_iter()
        .zip([
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
        ])
        .enumerate()
    {
        gui.app.press_key_modifiers(egui::Modifiers::CTRL, key);
        gui.settle();
        assert_eq!(gui.app.state().devices[0].page, page);
        gui.app
            .get_by_label(&format!("{} [{}]", page.name(), index + 1));
    }
    gui.app
        .press_key_modifiers(egui::Modifiers::CTRL, egui::Key::Tab);
    gui.settle();
    assert_eq!(gui.app.state().devices[0].page, Page::Overview);
    gui.app.press_key_modifiers(
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Key::Tab,
    );
    gui.settle();
    assert_eq!(gui.app.state().devices[0].page, Page::Firmware);
    gui.click("Console [2]");
    assert_eq!(gui.app.state().devices[0].page, Page::Console);
    assert!(gui.app.get_by_label("Console input").is_focused());
}

#[test]
fn console_entry_focus_enter_and_history_work_without_focus_clicks() {
    let mut gui = Gui::new(Page::Console, [1360., 940.], 0);
    assert!(gui.app.get_by_label("Console input").is_focused());
    gui.app.get_by_label("Console input").type_text("hi");
    gui.settle();
    gui.app.press_key(egui::Key::Enter);
    gui.settle();
    assert!(
        matches!(gui.commands.try_recv().unwrap(), live::Command::DataSend(bytes) if bytes == b"hi\n")
    );
    gui.events.send(live::Event::DataComplete).unwrap();
    gui.settle();
    assert!(gui.app.get_by_label("Console input").is_focused());
    gui.app.get_by_label("Console input").type_text("draft");
    gui.settle();
    gui.app.press_key(egui::Key::ArrowUp);
    gui.settle();
    assert_eq!(gui.app.state().devices[0].input, "hi");
    gui.app.press_key(egui::Key::ArrowDown);
    gui.settle();
    assert_eq!(gui.app.state().devices[0].input, "draft");
    gui.app.get_by_label("Search").focus();
    gui.settle();
    assert!(
        gui.app.get_by_label("Search").is_focused(),
        "Repaint must not steal focus"
    );
    gui.click("Overview [1]");
    gui.click("Console [2]");
    assert!(gui.app.get_by_label("Console input").is_focused());
    assert_eq!(gui.app.state().devices[0].input, "draft");
    gui.app.state_mut().show_history = true;
    gui.settle();
    gui.app.state_mut().show_history = false;
    gui.settle();
    assert!(gui.app.get_by_label("Console input").is_focused());
}

#[test]
fn console_sends_a_bare_line_ending_from_an_empty_input() {
    let mut gui = Gui::new(Page::Console, [1360., 940.], 0);
    assert!(gui.app.get_by_label("Console input").is_focused());
    // newline defaults to LF, so Enter with an empty input sends a lone LF.
    gui.app.press_key(egui::Key::Enter);
    gui.settle();
    assert!(
        matches!(
            gui.commands.try_recv().unwrap(),
            live::Command::DataSend(bytes) if bytes == b"\n"
        ),
        "empty input + LF must send a bare line ending"
    );
    gui.events.send(live::Event::DataComplete).unwrap();
    gui.settle();
    // A bare CRLF is sent when CRLF is selected; nothing is sent for None.
    gui.app.state_mut().devices[0].newline = 2;
    gui.settle();
    gui.app.press_key(egui::Key::Enter);
    gui.settle();
    assert!(
        matches!(
            gui.commands.try_recv().unwrap(),
            live::Command::DataSend(bytes) if bytes == b"\r\n"
        )
    );
    gui.events.send(live::Event::DataComplete).unwrap();
    gui.settle();
    gui.app.state_mut().devices[0].newline = 0;
    gui.settle();
    gui.app.press_key(egui::Key::Enter);
    gui.settle();
    assert!(
        gui.commands.try_recv().is_err(),
        "empty input with no line ending must not send"
    );
    assert_eq!(gui.app.state().devices[0].input, "");
}

#[test]
fn logic_channel_icons_rename_trigger_and_add_a_virtual_decoder_after_resize() {
    let mut gui = Gui::new(Page::Logic, [960., 700.], 0);
    gui.click("GP13");
    gui.app.get_by_label("Channel name").type_text("Host TX");
    gui.settle();
    assert_eq!(
        gui.app.state().devices[0]
            .logic
            .channels
            .iter()
            .find(|c| c.pin == 13)
            .unwrap()
            .name,
        "Host TX"
    );
    gui.click("Trigger on rising edge");
    assert_eq!(gui.app.state().devices[0].logic.trigger, 1);
    assert_eq!(gui.app.state().devices[0].logic.trigger_pin, 13);
    gui.app.set_size(egui::vec2(1360., 940.));
    gui.settle();
    gui.click("GP13");
    gui.click("Trigger on falling edge");
    assert_eq!(gui.app.state().devices[0].logic.trigger, 2);
    gui.click("GP13");
    gui.click("Add channel decoder");
    gui.click("UART");
    let device = &gui.app.state().devices[0];
    assert_eq!(device.logic.decoders.len(), 1);
    assert!(matches!(
        device.logic.decoders[0].decoder,
        logic::Decoder::Uart { pin: 13, .. }
    ));
    gui.app.get_by_label("UART · decode");
}

#[test]
fn logic_channel_popup_icons_align_in_both_languages() {
    for (size, language) in [([960., 700.], 0), ([1360., 940.], 1)] {
        let mut gui = Gui::new(Page::Logic, size, language);
        gui.click("GP13");
        let locale = &gui.app.state().locale;
        let mut previous: Option<egui::accesskit::Rect> = None;
        for key in [
            "No trigger",
            "Trigger on rising edge",
            "Trigger on falling edge",
            "Add channel decoder",
        ] {
            let bounds = gui.app.get_by_label(locale.tr(key)).raw_bounds().unwrap();
            assert!((bounds.height() - theme::CONTROL_HEIGHT as f64).abs() < 0.1);
            assert!(bounds.x0 >= 0. && bounds.x1 <= size[0] as f64);
            if let Some(other) = previous {
                assert!((bounds.y0 - other.y0).abs() < 0.1);
                assert!(bounds.x0 >= other.x1);
            }
            previous = Some(bounds);
        }
    }
}

#[test]
fn logic_selected_channel_supports_seconds_and_submits_its_memory_mask() {
    let mut gui = Gui::new(Page::Logic, [1360., 940.], 0);
    let logic = &mut gui.app.state_mut().devices[0].logic;
    logic.packed_capture = true;
    logic.select_channels(Some(false));
    logic.channels.iter_mut().find(|c| c.pin == 12).unwrap().enabled = true;
    logic.rate = 100000;
    logic.duration_s = 5.;
    gui.settle();
    assert_eq!(gui.app.state().devices[0].logic.max_samples(), 1310720);
    gui.click("Run");
    assert!(matches!(gui.commands.try_recv().unwrap(), live::Command::Capture {
        count: 500000, rate: 100000, channels, trigger: 0, ..
    } if channels == 1 << 12));
    gui.click("Stop");
}

#[test]
fn logic_run_stop_submit_the_selected_duration_and_trigger() {
    let mut gui = Gui::new(Page::Logic, [1360., 940.], 0);
    let logic = &mut gui.app.state_mut().devices[0].logic;
    logic.rate = 5_000_000;
    logic.duration_s = 0.002;
    logic.trigger = 2;
    logic.trigger_pin = 5;
    logic.trigger_timeout_ms = 750;
    gui.settle();
    gui.click("Run");
    assert!(matches!(
        gui.commands.try_recv().unwrap(),
        live::Command::Capture {
            channels: 0,
            count: 10_000,
            rate: 5_000_000,
            trigger: 2,
            pin: 5,
            timeout_ms: 750
        }
    ));
    gui.click("Stop");
    assert!(matches!(
        gui.commands.try_recv().unwrap(),
        live::Command::CaptureCancel
    ));
    assert!(gui.app.state().devices[0].logic.capturing.is_none());
}

#[test]
fn console_baud_apply_and_protocol_confirmation_use_the_selected_preset() {
    let mut gui = Gui::new(Page::Console, [960., 700.], 0);
    gui.click("UART baud");
    gui.click("230400 baud");
    gui.click("Apply");
    assert!(matches!(
        gui.commands.try_recv().unwrap(),
        live::Command::Baud(230400)
    ));
    assert!(gui.app.state().devices[0].baud_pending);
    assert!(gui.app.get_by_label("Apply").is_disabled());
    gui.events
        .send(live::Event::ConsoleState {
            framed: false,
            baud: 230400,
        })
        .unwrap();
    gui.events.send(live::Event::BaudComplete).unwrap();
    gui.settle();
    assert_eq!(gui.app.state().devices[0].baud, 230400);
    assert!(!gui.app.state().devices[0].baud_pending);
}

#[test]
fn overview_pause_freezes_only_the_view_and_resume_shows_latest_measurement() {
    let mut gui = Gui::new(Page::Overview, [1360., 940.], 0);
    let sample = |power_mw| live::Event::Measurement {
        value: ph_app_link::protocol::Measurement {
            shunt_uv: 50.,
            bus_mv: 5000.,
            current_ma: power_mw / 5.,
            power_mw,
        },
        captured_at: Instant::now(),
        timestamp_unix_ms: ph_app::telemetry::unix_ms(),
    };
    gui.events.send(sample(100.)).unwrap();
    gui.settle();
    gui.click("Pause refresh");
    gui.events.send(sample(150.)).unwrap();
    gui.settle();
    let monitor = &gui.app.state().devices[0].monitor;
    assert!(monitor.paused());
    assert_eq!(monitor.live.back().unwrap().power_mw, 150.);
    assert_eq!(monitor.samples(1.).last().unwrap().power_mw, 100.);
    gui.click("Resume refresh");
    let monitor = &gui.app.state().devices[0].monitor;
    assert!(!monitor.paused());
    assert_eq!(monitor.samples(1.).last().unwrap().power_mw, 150.);
}

#[test]
fn overview_wheel_and_drag_navigate_charts_and_fit_restores_live() {
    use ph_app::telemetry::Sample;
    let mut gui = Gui::new(Page::Overview, [960., 700.], 0);
    for i in 0..120 {
        gui.app.state_mut().devices[0].monitor.push(Sample {
            timestamp_unix_ms: i * 500,
            elapsed_s: i as f64 * 0.5,
            bus_voltage_v: 5.,
            work_3v3_v: 3.3,
            current_ma: 20. + i as f64 / 10.,
            power_mw: 100. + i as f64 / 2.,
            power_on: true,
            power_fault: false,
        });
    }
    gui.app.state_mut().devices[0].monitor.set_paused(true);
    gui.settle();
    let bounds = gui
        .app
        .get_by_label("Measurement charts")
        .raw_bounds()
        .unwrap();
    assert!(bounds.y1 < 700.);
    let pointer = egui::pos2(
        bounds.x0 as f32 + bounds.width() as f32 * 0.65,
        bounds.y0 as f32 + bounds.height() as f32 / 6.,
    );
    gui.app.input_mut().events.extend([
        egui::Event::PointerMoved(pointer),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0., 120.),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    gui.settle();
    let zoomed = gui.app.state().devices[0].monitor.viewport.time.unwrap();
    assert!(zoomed.1 - zoomed.0 < 60.);
    assert_eq!(gui.app.state().devices[0].monitor.viewport.values, [None; 3]);
    assert!(gui.app.state().devices[0].monitor.viewport.values[1].is_none());
    assert_eq!(
        gui.app
            .get_by_label("Measurement charts")
            .raw_bounds()
            .unwrap()
            .y0,
        bounds.y0,
        "Wheel must zoom the plot without scrolling the page"
    );
    // Wait for egui's finite scroll smoothing to drain before dragging.
    gui.app.run_steps(50);
    let before_drag = gui.app.state().devices[0].monitor.viewport.time.unwrap();
    gui.app.input_mut().events.push(egui::Event::PointerButton {
        pos: pointer,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    gui.settle();
    gui.app
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pointer + egui::vec2(70., 12.)));
    gui.settle();
    gui.app.input_mut().events.push(egui::Event::PointerButton {
        pos: pointer + egui::vec2(70., 12.),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    gui.settle();
    let dragged = gui.app.state().devices[0].monitor.viewport.time.unwrap();
    assert!(dragged.0 < before_drag.0);
    assert_eq!(gui.app.state().devices[0].monitor.viewport.values, [None; 3]);
    assert!(((dragged.1 - dragged.0) - (before_drag.1 - before_drag.0)).abs() < 1e-6);
    assert!(gui.app.state().devices[0].monitor.paused());
    gui.click("Fit charts");
    let monitor = &gui.app.state().devices[0].monitor;
    assert!(monitor.viewport.time.is_none());
    assert!(monitor.viewport.values.iter().all(Option::is_none));
    assert!(monitor.paused());
    gui.click("Resume refresh");
    assert!(!gui.app.state().devices[0].monitor.paused());
    gui.app.set_size(egui::vec2(1360., 940.));
    gui.settle();
    let taller = gui
        .app
        .get_by_label("Measurement charts")
        .raw_bounds()
        .unwrap();
    assert!(taller.height() > bounds.height());
    assert!(taller.y1 < 940.);
}

#[test]
fn swd_memory_refresh_accepts_bare_hex_and_the_selected_byte_count() {
    let mut gui = Gui::new(Page::Swd, [960., 700.], 0);
    {
        let d = &mut gui.app.state_mut().devices[0];
        d.swd = true;
        d.running = false;
        d.swd_tab = 1;
        d.mem_read_length = 1;
    }
    gui.settle();
    gui.app.get_by_label("Memory address").focus();
    gui.settle();
    gui.app
        .press_key_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    gui.app.get_by_label("Memory address").type_text("20041FD0");
    gui.settle();
    gui.app.press_key(egui::Key::Enter);
    gui.settle();
    assert!(
        matches!(gui.commands.try_recv().unwrap(), live::Command::MemoryRead {
        address: 0x20041FD0, length: 1, target,
    } if target == "rp2040")
    );
}

#[test]
fn console_connection_icons_and_auto_scroll_have_accessible_names() {
    let mut gui = Gui::new(Page::Console, [960., 700.], 0);
    for name in ["Hardware", "UART", "SPI"] {
        assert!(gui
            .app
            .get_all(by().role(egui::accesskit::Role::Label).label(name))
            .next()
            .is_some());
    }
    assert!(!gui.app.state().devices[0].paused);
    gui.click("Auto-scroll to bottom");
    assert!(gui.app.state().devices[0].paused);
    gui.click("Auto-scroll to bottom");
    assert!(!gui.app.state().devices[0].paused);
}

#[test]
fn header_icon_bounds_align_and_remain_inside_the_window() {
    for (size, language) in [([960., 700.], 0), ([1360., 940.], 0), ([960., 700.], 1)] {
        let mut gui = Gui::new(Page::Console, size, language);
        for sidebar in [false, true] {
            gui.app.state_mut().sidebar_open = sidebar;
            gui.settle();
            let app = gui.app.state();
            let mut previous: Option<egui::accesskit::Rect> = None;
            for key in ["Devices", "Power off", "Reset", "Scan devices", "Settings"] {
                let bounds = gui
                    .app
                    .get(
                        by().role(egui::accesskit::Role::Button)
                            .label(app.locale.tr(key)),
                    )
                    .raw_bounds()
                    .unwrap();
                assert!((bounds.height() - theme::CONTROL_HEIGHT as f64).abs() < 0.1);
                assert!(bounds.x0 >= 0. && bounds.x1 <= size[0] as f64);
                if let Some(other) = previous {
                    assert!((bounds.y0 - other.y0).abs() < 0.1);
                    assert!(bounds.x1 <= other.x0 || bounds.x0 >= other.x1);
                }
                previous = Some(bounds);
            }
        }
    }
}
