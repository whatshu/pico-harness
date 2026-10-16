#[cfg(test)]
use crate::model::TaskKind;
use crate::{
    export, file_pick,
    i18n::I18n,
    live,
    model::{push, Device, Page},
    panel, ACCENT, HOT, MUTED, RED,
};
use eframe::egui::{self, Color32, RichText};

pub fn baud_preset(actual: u32) -> u32 {
    BAUD_PRESETS
        .into_iter()
        .find(|baud| actual.abs_diff(*baud) <= baud / 50)
        .unwrap_or(actual)
}
pub const BAUD_PRESETS: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
pub fn console_input_id(d: &Device) -> egui::Id {
    egui::Id::new(("console-input", &d.id))
}
pub fn show(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64, notice: &mut String) {
    match d.page {
        Page::Overview => crate::monitor_ui::show(ui, d, l, now, notice),
        Page::Console => console(ui, d, l, now, notice),
        Page::Swd => live_swd(ui, d, l, notice),
        Page::Firmware => firmware(ui, d, l, now, notice),
        _ => {}
    }
}
fn console(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64, notice: &mut String) {
    let mut resume_scroll = false;
    let can_send = d.available()
        && !d.baud_pending
        && !d.task.as_ref().is_some_and(|task| task.status == "Running");
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(l.tr("Work Console")).color(ACCENT));
        ui.label(
            RichText::new(if d.console_framed {
                "UART + SPI"
            } else {
                "UART"
            })
            .color(MUTED),
        );
        ui.add_enabled_ui(!d.baud_pending, |ui| {
            let baud = egui::ComboBox::from_id_salt("console-baud")
                .selected_text(format!("{} baud", d.baud_draft))
                .show_ui(ui, |ui| {
                    for baud in BAUD_PRESETS {
                        ui.selectable_value(&mut d.baud_draft, baud, format!("{baud} baud"));
                    }
                });
            crate::theme::accessible_label(&baud.response, l.tr("UART baud"));
        });
        if crate::theme::button(
            ui,
            l.tr("Apply"),
            56.,
            d.available() && !d.baud_pending && d.baud_draft != baud_preset(d.baud),
        )
        .clicked()
        {
            match d.request_live(live::Command::Baud(d.baud_draft)) {
                Ok(()) => d.baud_pending = true,
                Err(error) => *notice = error,
            }
        }
        if d.baud_pending {
            ui.spinner();
        }
        ui.separator();
        ui.checkbox(&mut d.hex, "HEX");
        if crate::theme::icon_toggle(
            ui,
            crate::theme::Icon::ScrollToBottom,
            l.tr("Auto-scroll to bottom"),
            !d.paused,
            true,
        )
        .clicked()
        {
            d.paused = !d.paused;
            resume_scroll = !d.paused;
        }
        let search = ui.add(
            egui::TextEdit::singleline(&mut d.search)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .hint_text(l.tr("Search"))
                .desired_width(160.),
        );
        crate::theme::accessible_label(&search, l.tr("Search"));
        if crate::theme::icon_button(ui, crate::theme::Icon::Trash, l.tr("Clear"), true).clicked() {
            d.logs.clear();
        }
        if crate::theme::icon_button(
            ui,
            crate::theme::Icon::Export,
            l.tr("Export"),
            !d.logs.is_empty(),
        )
        .clicked()
        {
            let text = d
                .logs
                .iter()
                .map(|e| format!("{:.3}\t{}\t{}", e.time, e.direction, e.text))
                .collect::<Vec<_>>()
                .join("\n");
            if let Err(e) = export("uart.log", text) {
                *notice = e;
            }
        }
        if crate::theme::icon_button(ui, crate::theme::Icon::Trace, l.tr("Transport trace"), true)
            .clicked()
        {
            d.trace_open = true;
        }
    });
    ui.separator();
    console_status(ui, d, l);
    let height = (ui.available_height() - 110.).max(100.);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0_f32, crate::BORDER))
        .inner_margin(8)
        .corner_radius(2)
        .show(ui, |ui| {
            let search = d.search.to_lowercase();
            let mut rows = Vec::new();
            for log in d
                .logs
                .iter()
                .filter(|log| search.is_empty() || log.text.to_lowercase().contains(&search))
            {
                if d.hex {
                    rows.push((log, None, true));
                } else {
                    let text = log.text.trim_end_matches(['\r', '\n']);
                    for (index, line) in text.split('\n').enumerate() {
                        rows.push((log, Some(line.trim_end_matches('\r')), index == 0));
                    }
                }
            }
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            let mut scroll = egui::ScrollArea::both()
                .stick_to_bottom(!d.paused)
                .auto_shrink([false, false])
                .max_height(height)
                .min_scrolled_height(height);
            if resume_scroll {
                scroll = scroll.vertical_scroll_offset(rows.len() as f32 * row_height);
            }
            scroll.show_rows(ui, row_height, rows.len().max(1), |ui, visible| {
                if d.logs.is_empty() {
                    ui.label(
                        RichText::new(l.tr("Waiting for UART data"))
                            .small()
                            .color(MUTED),
                    );
                }
                for &(e, line, first) in rows.get(visible).unwrap_or_default() {
                    ui.horizontal_top(|ui| {
                        ui.add_sized(
                            [88., 14.],
                            egui::Label::new(
                                RichText::new(if first {
                                    format!("{:09.3}", e.time)
                                } else {
                                    String::new()
                                })
                                .monospace()
                                .color(MUTED),
                            ),
                        );
                        ui.add_sized(
                            [24., 14.],
                            egui::Label::new(
                                RichText::new(if first { &e.direction } else { "" })
                                    .monospace()
                                    .color(if e.direction == "RX" {
                                        ACCENT
                                    } else {
                                        Color32::LIGHT_BLUE
                                    }),
                            ),
                        );
                        let text: std::borrow::Cow<'_, str> = if d.hex {
                            e.raw
                                .as_deref()
                                .unwrap_or(e.text.as_bytes())
                                .iter()
                                .map(|b| format!("{b:02X} "))
                                .collect::<String>()
                                .into()
                        } else {
                            line.unwrap_or_default().into()
                        };
                        ui.add(egui::Label::new(RichText::new(text.as_ref()).monospace()).extend());
                    });
                }
            });
        });
    ui.add_space(8.);
    ui.horizontal(|ui| {
        let input_id = console_input_id(d);
        let response = ui.add(
            egui::TextEdit::singleline(&mut d.input)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .hint_text(l.tr("Send a command to Work Core…"))
                .id(input_id)
                .desired_width((ui.available_width() - 160.).max(80.)),
        );
        crate::theme::accessible_label(&response, l.tr("Console input"));
        response
            .clone()
            .on_hover_text(l.tr("↑/↓ recall commands; ↓ restores your draft"));
        let mut recalled = false;
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                d.history.previous(&mut d.input);
                recalled = true;
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
            {
                d.history.next(&mut d.input);
                recalled = true;
            }
        }
        if recalled {
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(d.input.chars().count()),
                    )));
                state.store(ui.ctx(), response.id);
            }
        }
        egui::ComboBox::from_id_salt("newline")
            .selected_text(["None", "LF", "CRLF"][d.newline])
            .show_ui(ui, |ui| {
                for (i, s) in ["None", "LF", "CRLF"].iter().enumerate() {
                    ui.selectable_value(&mut d.newline, i, *s);
                }
            });
        let send = ui
            .add_enabled(can_send, egui::Button::new(l.tr("Send")))
            .clicked()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if send && can_send {
            let end = ["", "\n", "\r\n"][d.newline];
            if d.input.is_empty() && end.is_empty() {
                // Empty input with no line ending selected: nothing to send.
                response.request_focus();
            } else {
                let mut text = std::mem::take(&mut d.input);
                if !text.is_empty() {
                    d.history.remember(&text);
                }
                text.push_str(end);
                d.start_live_data_send(text.into_bytes(), now);
                response.request_focus();
            }
        }
    });
    crate::theme::row(ui, |ui| {
        let busy = d.task.as_ref().is_some_and(|task| task.status == "Running");
        if ui
            .add_enabled(d.available() && !busy, egui::Button::new(l.tr("Send file")))
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                match std::fs::metadata(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|metadata| {
                        if metadata.len() > 16 * 1024 * 1024 {
                            return Err("File exceeds 16 MiB".into());
                        }
                        std::fs::read(path).map_err(|e| e.to_string())
                    }) {
                    Ok(bytes) if !bytes.is_empty() => d.start_live_data_send(bytes, now),
                    Ok(_) => *notice = l.tr("Payload is empty").into(),
                    Err(error) => *notice = error,
                }
            }
        }
        if ui
            .add_enabled(
                d.available() && !busy && d.console_framed,
                egui::Button::new(l.tr("Receive")),
            )
            .clicked()
        {
            d.start_live_data_receive(65536, now);
        }
        if ui
            .add_enabled(
                !d.received.is_empty(),
                egui::Button::new(l.tr("Save received data")),
            )
            .clicked()
        {
            if let Err(error) = export("console-received.bin", &d.received) {
                *notice = error;
            }
        }
        if let Some(task) = &d.task {
            ui.label(format!(
                "{} · {}  {}/{} B · {:.1} KB/s",
                l.tr(match task.kind {
                    crate::model::TaskKind::Send => "Send",
                    crate::model::TaskKind::Receive => "Receive",
                }),
                l.tr(&task.status),
                task.transferred,
                task.total,
                task.speed_kb_s(now)
            ));
        }
    });
    let mut trace_open = d.trace_open;
    egui::Window::new(l.tr("Transport trace"))
        .id(egui::Id::new(("transport-trace", &d.id)))
        .open(&mut trace_open)
        .default_size([740., 440.])
        .show(ui.ctx(), |ui| {
            crate::theme::row(ui, |ui| {
                ui.label(l.tr("Read-only UART / SPI trace"));
                if ui.button(l.tr("Export")).clicked() {
                    let mut csv = String::from("elapsed_s,transport,direction,bytes,hex,decoded\n");
                    for trace in &d.trace {
                        let hex: String = trace
                            .bytes
                            .iter()
                            .map(|byte| format!("{byte:02X}"))
                            .collect();
                        csv.push_str(&format!(
                            "{:.6},{},{},{},{},\"{}\"\n",
                            trace.time,
                            trace.bus,
                            trace.direction,
                            trace.bytes.len(),
                            hex,
                            trace.decoded.replace('"', "\"\"")
                        ));
                    }
                    if let Err(error) = export("transport-trace.csv", csv) {
                        *notice = error;
                    }
                }
            });
            let count = d.trace.len().min(500);
            let start = d.trace.len() - count;
            let height = ui.text_style_height(&egui::TextStyle::Body)
                + ui.text_style_height(&egui::TextStyle::Monospace)
                + ui.spacing().item_spacing.y;
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show_rows(ui, height, count, |ui, visible| {
                    for trace in d
                        .trace
                        .iter()
                        .skip(start + visible.start)
                        .take(visible.len())
                    {
                        let hex: String = trace
                            .bytes
                            .iter()
                            .take(64)
                            .map(|byte| format!("{byte:02X} "))
                            .collect();
                        ui.horizontal_top(|ui| {
                            ui.monospace(format!(
                                "{:09.3}  {:4} {}  {:5} B",
                                trace.time,
                                trace.bus,
                                trace.direction,
                                trace.bytes.len()
                            ));
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(trace.decoded.replace('\n', " · ")).truncate(),
                                )
                                .on_hover_text(&trace.decoded);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "{}{}",
                                            hex,
                                            if trace.bytes.len() > 64 { "…" } else { "" }
                                        ))
                                        .monospace()
                                        .color(MUTED),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                    }
                });
        });
    d.trace_open = trace_open;
}
fn console_status(ui: &mut egui::Ui, d: &Device, l: &I18n) {
    use crate::theme::{status_icon, Icon};
    crate::theme::row(ui, |ui| {
        status_icon(
            ui,
            Icon::Hardware,
            l.tr("Hardware"),
            &format!(
                "{}\nWork {} · {:.3} V",
                l.tr(if d.live_ready {
                    "USB responding"
                } else {
                    "Waiting for device"
                }),
                l.tr(if d.power { "Power on" } else { "Power off" }),
                d.voltage,
            ),
            if d.live_ready { ACCENT } else { MUTED },
        );
        status_icon(
            ui,
            Icon::Uart,
            "UART",
            &format!(
                "{}\n{} baud · 8N1 · GP12/13",
                l.tr(if d.work_verified {
                    "Protocol verified"
                } else if d.console_framed {
                    "Framed console detected"
                } else {
                    "Plain UART / protocol unconfirmed"
                }),
                d.baud,
            ),
            if d.uart_active() {
                HOT
            } else if d.work_verified {
                ACCENT
            } else {
                MUTED
            },
        );
        status_icon(
            ui,
            Icon::Spi,
            "SPI",
            &format!(
                "{}\n{}",
                l.tr(if d.work_verified {
                    "Automatic bulk supported"
                } else {
                    "Protocol unconfirmed"
                }),
                l.tr(if d.work_bulk_ready {
                    "Work bulk data ready · GP18–21"
                } else {
                    "GP18–21 · idle / no pending bulk"
                }),
            ),
            if d.bulk_active() {
                HOT
            } else if d.work_verified {
                ACCENT
            } else {
                MUTED
            },
        );
    });
    ui.add_space(5.);
}
#[cfg(test)]
fn progress(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64) {
    if let Some(t) = &d.task {
        panel().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(l.tr(&t.status)).color(if t.status == "Failed" {
                        RED
                    } else {
                        ACCENT
                    }),
                );
                ui.label(format!("{} / {} bytes", t.transferred, t.total));
            });
            ui.label(l.tr(match t.kind {
                TaskKind::Send => "Send",
                TaskKind::Receive => "Receive",
            }));
            ui.add(egui::ProgressBar::new(t.progress).show_percentage());
            ui.label(format!("{:.1} KB/s", t.speed_kb_s(now)));
            if t.status == "Failed" {
                ui.colored_label(RED, &d.live_error);
            }
        });
    }
}
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let text = text.replace([' ', '\n', '\r', '\t'], "");
    if text.len() % 2 != 0 || !text.is_ascii() {
        return Err("HEX requires complete byte pairs".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
fn firmware(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64, notice: &mut String) {
    let busy = d.flash.job.is_some();
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(!busy, |ui| {
            ui.label(l.tr("Firmware image"));
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.firmware)
                        .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                        .hint_text(".uf2 / .elf / .bin")
                        .desired_width((ui.available_width() - 100.).max(100.)),
                );
                if ui.button(l.tr("Browse")).clicked() {
                    if let Some(path) = file_pick(&["uf2", "elf", "bin"]) {
                        d.firmware = path;
                    }
                }
            });
            ui.label(format!("{}: {} / Work Core", l.tr("Target"), d.name));
            programmer_settings(ui, d, l);
            ui.checkbox(&mut d.reset_after, l.tr("Reset after programming"));
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy && !d.firmware.is_empty(),
                    egui::Button::new(l.tr("Program firmware")),
                )
                .clicked()
            {
                if let Err(error) = d.start_firmware(now) {
                    *notice = l.tr(&error).into();
                }
            }
            if busy && ui.button(l.tr("Cancel")).clicked() {
                d.cancel_firmware();
            }
            let color = if d.flash.error.is_empty() {
                ACCENT
            } else {
                RED
            };
            ui.label(RichText::new(l.tr(&d.flash.status)).color(color));
            if busy {
                ui.spinner();
            }
        });
        if !d.flash.format.is_empty() {
            ui.monospace(format!("{} · {} bytes", d.flash.format, d.flash.bytes));
        }
        if !d.flash.error.is_empty() {
            ui.colored_label(RED, l.tr(&d.flash.error));
        }
    });
    ui.label(
        RichText::new(l.tr(
            "Drop UF2 / ELF / BIN here to program Work Core. Elsewhere, dropping ELF starts GDB.",
        ))
        .small()
        .color(MUTED),
    );
    ui.add_space(12.);
    crate::theme::terminal(
        ui,
        ("program-log", &d.id),
        &d.flash.logs,
        ui.available_height(),
    );
}

fn programmer_settings(ui: &mut egui::Ui, d: &mut Device, l: &I18n) {
    ui.horizontal_wrapped(|ui| {
        ui.label(l.tr("Work chip"));
        egui::ComboBox::from_id_salt("flash-target")
            .selected_text(&d.debugger.config.target)
            .show_ui(ui, |ui| {
                for target in ["rp2040", "rp2350"] {
                    ui.selectable_value(&mut d.debugger.config.target, target.into(), target);
                }
            });
        ui.label(l.tr("Probe serial"));
        ui.monospace(&d.debugger.config.serial);
    });
    ui.collapsing(l.tr("Programmer settings"), |ui| {
        for (label, value) in [
            ("OpenOCD", &mut d.debugger.config.openocd),
            ("Scripts directory", &mut d.debugger.config.scripts),
        ] {
            ui.horizontal(|ui| {
                ui.label(l.tr(label));
                crate::theme::text_field(ui, value);
            });
        }
        if d.id.starts_with("remote-") {
            ui.horizontal(|ui| {
                ui.label(l.tr("Probe serial"));
                crate::theme::text_field(ui, &mut d.debugger.config.serial);
            });
        }
    });
    ui.label(RichText::new(l.tr("Work power is enabled automatically. Successful programming leaves Work powered; failures restore previously disabled power.")).small().color(MUTED));
}

fn live_swd(ui: &mut egui::Ui, d: &mut Device, l: &I18n, notice: &mut String) {
    panel().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&d.debugger.config.target).strong());
            if d.swd_idcode != 0 {
                ui.monospace(format!("IDCODE 0x{:08X}", d.swd_idcode));
            }
            let previous = d.core;
            for core in 0..2 {
                ui.selectable_value(&mut d.core, core, format!("Core {core}"));
            }
            if d.core != previous && d.swd {
                d.registers = d.core_registers[d.core];
                d.previous = d.registers;
                d.reg_drafts = std::array::from_fn(|i| {
                    if d.registers_valid[d.core] {
                        format!("{:08X}", d.registers[i])
                    } else {
                        String::new()
                    }
                });
                d.swd_pending = true;
                if let Err(error) = d.request_live(live::Command::SwdConnect {
                    target: d.debugger.config.target.clone(),
                    core: d.core as u8,
                }) {
                    *notice = error;
                    d.swd_pending = false;
                }
            }
            if ui
                .add_enabled(
                    d.available() && !d.swd && !d.swd_pending && d.debugger.session.is_none(),
                    egui::Button::new(l.tr("Connect")),
                )
                .clicked()
            {
                d.swd_pending = true;
                if let Err(error) = d.request_live(live::Command::SwdConnect {
                    target: d.debugger.config.target.clone(),
                    core: d.core as u8,
                }) {
                    *notice = error;
                    d.swd_pending = false;
                }
            }
            if ui
                .add_enabled(d.swd, egui::Button::new(l.tr("Disconnect")))
                .clicked()
            {
                if let Err(error) = d.request_live(live::Command::SwdDisconnect) {
                    *notice = error;
                }
            }
            for (label, command) in [
                (
                    if d.running { "Halt" } else { "Resume" },
                    if d.running { "halt" } else { "resume" },
                ),
                ("Reset", "reset"),
                ("Refresh", "reg"),
            ] {
                let active = d.swd
                    && (command != "reset" || d.available())
                    && (command != "reg" || !d.running);
                if ui
                    .add_enabled(active, egui::Button::new(l.tr(label)))
                    .clicked()
                {
                    if command == "reset" {
                        if let Err(error) = d.request_live(live::Command::Reset) {
                            *notice = error;
                        }
                    } else if let Err(error) = d.request_live(live::Command::Swd(command.into())) {
                        *notice = error;
                    }
                }
            }
        });
        if d.swd {
            ui.horizontal_wrapped(|ui| {
                for i in [15, 13, 14, 16] {
                    ui.label(
                        RichText::new(if d.registers_valid[d.core] {
                            format!("{}  {:08X}", reg_name(i), d.registers[i])
                        } else {
                            format!("{}  —", reg_name(i))
                        })
                        .monospace()
                        .color(if d.running {
                            MUTED
                        } else {
                            ui.visuals().text_color()
                        }),
                    );
                    ui.add_space(12.);
                }
            });
        }
    });
    ui.add_space(10.);
    ui.horizontal(|ui| {
        for (i, name) in ["Terminal", "Memory", "Registers"].iter().enumerate() {
            ui.selectable_value(&mut d.swd_tab, i, l.tr(name));
        }
    });
    ui.separator();
    match d.swd_tab {
        0 => {
            crate::theme::terminal(ui, "swd-terminal", &d.swd_logs, ui.available_height() - 38.);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut d.swd_input)
                        .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                        .font(egui::TextStyle::Monospace)
                        .desired_width((ui.available_width() - 90.).max(100.)),
                );
                let submit = ui
                    .add_enabled(d.swd, egui::Button::new(l.tr("Execute")))
                    .clicked()
                    || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if submit && d.swd && !d.swd_input.trim().is_empty() {
                    let command = std::mem::take(&mut d.swd_input);
                    push(&mut d.swd_logs, format!("> {command}"), 1000);
                    if let Err(error) = d.request_live(live::Command::Swd(command)) {
                        *notice = error;
                    }
                    response.request_focus();
                }
            });
        }
        1 => {
            ui.horizontal_wrapped(|ui| {
                use ph_app::memory_export;
                ui.label(l.tr("Address"));
                let address_input = ui.add(
                    egui::TextEdit::singleline(&mut d.mem_address)
                        .id_salt("swd-memory-address")
                        .desired_width(140.),
                );
                crate::theme::accessible_label(&address_input, l.tr("Memory address"));
                #[cfg(test)]
                ui.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new("memory-address-input"),
                        (address_input.id, address_input.rect),
                    )
                });
                ui.label(l.tr("Read length"));
                let length = ui.add(
                    egui::DragValue::new(&mut d.mem_read_length)
                        .range(1..=4 * 1024 * 1024)
                        .suffix(" bytes"),
                );
                crate::theme::accessible_label(&length, l.tr("Read length"));
                if ui
                    .add_enabled(d.swd, egui::Button::new(l.tr("Refresh")))
                    .clicked()
                    || (d.swd
                        && address_input.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    match memory_export::parse_address(&d.mem_address) {
                        Ok(address) => {
                            if let Err(error) = d.request_live(live::Command::MemoryRead {
                                address,
                                length: d.mem_read_length,
                                target: d.debugger.config.target.clone(),
                            }) {
                                *notice = error;
                            }
                        }
                        Err(_) => *notice = l.tr("Invalid hexadecimal address").into(),
                    }
                }
                if let Some(address) = d.live_memory_address {
                    ui.label(
                        RichText::new(format!("0x{address:08X} · {} bytes", d.memory.len()))
                            .small()
                            .color(MUTED),
                    );
                }
            });
            crate::theme::row(ui, |ui| {
                use ph_app::memory_export::{self, Format};
                ui.label(l.tr("Export length"));
                ui.add(
                    egui::DragValue::new(&mut d.mem_export_length)
                        .range(1..=4 * 1024 * 1024)
                        .suffix(" bytes"),
                );
                egui::ComboBox::from_id_salt("memory-export-format")
                    .selected_text(d.mem_export_format.name())
                    .show_ui(ui, |ui| {
                        for format in [Format::Binary, Format::IntelHex, Format::Csv] {
                            ui.selectable_value(&mut d.mem_export_format, format, format.name());
                        }
                    });
                if ui
                    .add_enabled(
                        d.swd && !d.running && d.mem_export_pending.is_none(),
                        egui::Button::new(l.tr("Export range")),
                    )
                    .clicked()
                {
                    let result = ph_app::memory_export::parse_address(&d.mem_address)
                        .map_err(|e| e.to_string())
                        .and_then(|address| {
                            memory_export::validate_range(address, d.mem_export_length)?;
                            let format = d.mem_export_format;
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(format!(
                                    "memory-{address:08x}.{}",
                                    format.extension()
                                ))
                                .add_filter(format.name(), &[format.extension()])
                                .save_file()
                            {
                                d.request_live(live::Command::MemoryExport {
                                    address,
                                    length: d.mem_export_length,
                                })?;
                                d.mem_export_pending = Some((path, format));
                            }
                            Ok(())
                        });
                    if let Err(error) = result {
                        *notice = error;
                    }
                }
            });
            if let Some(base) = d.live_memory_address {
                egui::ScrollArea::vertical()
                    .max_height(300.)
                    .show(ui, |ui| {
                        for (row, bytes) in d.memory.chunks(16).enumerate() {
                            let hex = bytes
                                .iter()
                                .map(|byte| format!("{byte:02X} "))
                                .collect::<String>();
                            let ascii = bytes
                                .iter()
                                .map(|byte| {
                                    if byte.is_ascii_graphic() {
                                        *byte as char
                                    } else {
                                        '.'
                                    }
                                })
                                .collect::<String>();
                            ui.monospace(format!(
                                "{:08X}   {:<48}  {}",
                                base + row as u32 * 16,
                                hex,
                                ascii
                            ));
                        }
                    });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(l.tr("Address"));
                ui.add(
                    egui::DragValue::new(&mut d.mem_offset)
                        .range(0..=252)
                        .speed(4.),
                );
                crate::theme::text_field(ui, &mut d.mem_edit);
                if ui
                    .add_enabled(d.swd && !d.running, egui::Button::new(l.tr("Write")))
                    .clicked()
                {
                    match (
                        ph_app::memory_export::parse_address(&d.mem_address),
                        parse_hex(&d.mem_edit),
                    ) {
                        (Ok(base), Ok(bytes))
                            if bytes.len() == 4 && (base + d.mem_offset as u32) & 3 == 0 =>
                        {
                            let address = base + d.mem_offset as u32;
                            let value = u32::from_le_bytes(bytes.try_into().unwrap());
                            if let Err(error) = d.request_live(live::Command::Swd(format!(
                                "mww 0x{address:08X} 0x{value:08X}"
                            ))) {
                                *notice = error;
                            }
                        }
                        _ => {
                            *notice =
                                "Write exactly four HEX bytes at a 4-byte aligned address".into()
                        }
                    }
                }
            });
        }
        _ => {
            crate::theme::row(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.reg_search)
                        .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                        .desired_width(230.)
                        .hint_text(l.tr("Search registers")),
                );
                if ui
                    .add_enabled(d.swd && !d.running, egui::Button::new(l.tr("Refresh")))
                    .clicked()
                {
                    if let Err(error) = d.request_live(live::Command::Swd("reg".into())) {
                        *notice = error;
                    }
                }
            });
            egui::ScrollArea::vertical().show(ui, |ui| {
                for i in 0..17 {
                    let name = reg_name(i);
                    if !name.to_lowercase().contains(&d.reg_search.to_lowercase()) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(44., crate::theme::CONTROL_HEIGHT),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(44.);
                                ui.monospace(&name);
                            },
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut d.reg_drafts[i])
                                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                                .id_salt(("live_reg", &d.id, d.core, i))
                                .font(egui::TextStyle::Monospace)
                                .hint_text("—")
                                .desired_width(140.),
                        );
                        if ui
                            .add_enabled(
                                d.swd && !d.running && i < 16,
                                egui::Button::new(l.tr("Write")),
                            )
                            .clicked()
                        {
                            if let Ok(value) =
                                u32::from_str_radix(d.reg_drafts[i].trim_start_matches("0x"), 16)
                            {
                                if let Err(error) = d.request_live(live::Command::Swd(format!(
                                    "reg {i} 0x{value:08X}"
                                ))) {
                                    *notice = error;
                                }
                            } else {
                                *notice = l.tr("Invalid register value").into();
                            }
                        }
                        egui::CollapsingHeader::new(l.tr("Bits"))
                            .id_salt(("live_register_bits", &d.id, d.core, i))
                            .show(ui, |ui| {
                                ui.monospace(if d.swd {
                                    format!("{:032b}", d.registers[i])
                                } else {
                                    "—".into()
                                });
                            });
                    });
                }
            });
        }
    }
}
pub fn reg_name(i: usize) -> String {
    match i {
        13 => "SP".into(),
        14 => "LR".into(),
        15 => "PC".into(),
        16 => "xPSR".into(),
        _ => format!("R{i}"),
    }
}
pub fn number(s: &str) -> Result<u32, std::num::ParseIntError> {
    let s = s.trim();
    if let Some(s) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(s, 16)
    } else {
        s.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console_frame(
        ctx: &egui::Context,
        d: &mut Device,
        key: Option<egui::Key>,
        focus: bool,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 600.),
                )),
                events: key
                    .into_iter()
                    .flat_map(|key| {
                        [true, false].map(|pressed| egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        })
                    })
                    .collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if focus {
                        ui.memory_mut(|memory| {
                            memory.request_focus(egui::Id::new(("console-input", &d.id)))
                        });
                    }
                    console(ui, d, &I18n::default(), 0., &mut String::new());
                });
            },
        )
    }

    #[test]
    fn console_arrow_history_keeps_draft_and_input_focus() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut d = Device::base("test".into(), "Probe".into());
        d.history.remember("first");
        d.history.remember("第二条命令");
        d.input = "unfinished".into();
        console_frame(&ctx, &mut d, None, true);
        console_frame(&ctx, &mut d, None, false);
        for (key, expected) in [
            (egui::Key::ArrowUp, "第二条命令"),
            (egui::Key::ArrowUp, "first"),
            (egui::Key::ArrowUp, "first"),
            (egui::Key::ArrowDown, "第二条命令"),
            (egui::Key::ArrowDown, "unfinished"),
            (egui::Key::ArrowDown, "unfinished"),
        ] {
            let output = console_frame(&ctx, &mut d, Some(key), false);
            assert_eq!(d.input, expected);
            let mut text = vec![];
            for shape in &output.shapes {
                text_shapes(&shape.shape, &mut text);
            }
            assert!(!text.iter().any(|text| text == "History"));
            assert!(ctx.memory(|memory| memory.focused()).is_some());
        }
        ctx.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        console_frame(&ctx, &mut d, Some(egui::Key::ArrowUp), false);
        assert_eq!(d.input, "unfinished");
    }

    fn interactive_frame(ctx: &egui::Context, device: &mut Device, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 800.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, device, &I18n::default(), 1., &mut String::new());
                });
            },
        );
    }

    #[test]
    fn baud_apply_submits_the_edited_value_from_the_real_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut device = Device::base("test".into(), "Probe".into());
        device.page = Page::Console;
        device.online = true;
        device.live_ready = true;
        device.power = true;
        let (session, events, commands) = live::Session::test_source();
        device.live_session = Some(session);
        device.baud_draft = 921600;
        events
            .send(live::Event::ConsoleState {
                framed: true,
                baud: 115200,
            })
            .unwrap();
        device.tick(1.);
        interactive_frame(&ctx, &mut device, vec![]);
        let position = ctx
            .data(|data| {
                data.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            })
            .unwrap()
            .into_iter()
            .find(|(label, _)| label == "Apply")
            .unwrap()
            .1
            .center();
        for pressed in [true, false] {
            interactive_frame(
                &ctx,
                &mut device,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(device.baud_pending);
        assert!(matches!(
            commands.try_recv().unwrap(),
            live::Command::Baud(921600)
        ));
    }

    #[test]
    fn memory_address_can_be_edited_and_submitted_with_enter() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut device = Device::base("test".into(), "Probe".into());
        device.page = Page::Swd;
        device.swd_tab = 1;
        device.swd = true;
        let (session, _events, commands) = live::Session::test_source();
        device.live_session = Some(session);
        interactive_frame(&ctx, &mut device, vec![]);
        let (id, _) = ctx
            .data(|data| {
                data.get_temp::<(egui::Id, egui::Rect)>(egui::Id::new("memory-address-input"))
            })
            .unwrap();
        ctx.memory_mut(|memory| memory.request_focus(id));
        interactive_frame(
            &ctx,
            &mut device,
            vec![
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        ctrl: true,
                        command: true,
                        ..egui::Modifiers::NONE
                    },
                },
                egui::Event::Text("0X20000100".into()),
            ],
        );
        assert_eq!(device.mem_address, "0X20000100");
        interactive_frame(
            &ctx,
            &mut device,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(matches!(
            commands.try_recv().unwrap(),
            live::Command::MemoryRead {
                address: 0x20000100,
                length: 256,
                ..
            }
        ));
    }

    #[test]
    #[ignore = "Uses the selected RP2040 probe; temporarily halts and restores core 0"]
    fn connected_sram_end_memory_refresh() {
        use std::time::{Duration, Instant};
        fn wait(d: &Device, accept: impl Fn(&live::Event) -> bool) -> Result<live::Event, String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let event = d
                    .live_session
                    .as_ref()
                    .unwrap()
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|e| e.to_string())?;
                if accept(&event) {
                    return Ok(event);
                }
                if let live::Event::Error { operation, message } = event {
                    return Err(format!("{operation}: {message}"));
                }
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let serial = std::env::var("PDBG_PROBE_SERIAL").unwrap();
        let mut d = Device::base(serial.clone(), "Probe".into());
        d.live_session = Some(live::Session::start(serial, 115200, false));
        d.request_live(live::Command::SwdConnect {
            target: "rp2040".into(),
            core: 0,
        })
        .unwrap();
        let initially_halted = matches!(
            wait(&d, |event| matches!(
                event,
                live::Event::SwdState {
                    connected: true,
                    ..
                }
            ))
            .unwrap(),
            live::Event::SwdState { halted: true, .. }
        );
        let result = (|| -> Result<(), String> {
            if !initially_halted {
                d.request_live(live::Command::Swd("halt".into()))?;
                wait(&d, |event| matches!(event, live::Event::SwdRegisters(_)))?;
            }
            // Establish that this address is readable independently of the UI.
            for count in [1, 12] {
                d.request_live(live::Command::Swd(format!("mdw 0x20041FD0 {count}")))?;
                if let live::Event::SwdMemory { address, bytes } =
                    wait(&d, |event| matches!(event, live::Event::SwdMemory { .. }))?
                {
                    if address != 0x20041FD0 || bytes.len() != count * 4 {
                        return Err("Unexpected SWD memory response".into());
                    }
                    eprintln!("0x20041FD0: {} bytes read successfully", bytes.len());
                }
            }
            d.page = Page::Swd;
            d.swd_tab = 1;
            d.swd = true;
            for (text, address, length, expected) in [
                ("0x20041FD0", 0x20041FD0, 256, 48),
                ("20041FD0", 0x20041FD0, 1, 1),
                ("20041FFF", 0x20041FFF, 256, 1),
            ] {
                d.mem_address = text.into();
                d.mem_read_length = length;
                interactive_frame(&ctx, &mut d, vec![]);
                let (id, _) = ctx
                    .data(|data| {
                        data.get_temp::<(egui::Id, egui::Rect)>(egui::Id::new(
                            "memory-address-input",
                        ))
                    })
                    .unwrap();
                ctx.memory_mut(|memory| memory.request_focus(id));
                for pressed in [true, false] {
                    interactive_frame(
                        &ctx,
                        &mut d,
                        vec![egui::Event::Key {
                            key: egui::Key::Enter,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                if let live::Event::SwdMemory {
                    address: actual,
                    bytes,
                } = wait(&d, |event| matches!(event, live::Event::SwdMemory { .. }))?
                {
                    if actual != address || bytes.len() != expected {
                        return Err(format!(
                            "Expected {expected} bytes at 0x{address:08X}, got {}",
                            bytes.len()
                        ));
                    }
                    eprintln!(
                        "Memory page {text}, requested {length}: {} bytes read successfully",
                        bytes.len()
                    );
                }
            }
            d.request_live(live::Command::MemoryExport {
                address: 0x20041FD0,
                length: 1,
            })?;
            if let live::Event::MemoryExport { bytes, .. } = wait(&d, |event| {
                matches!(event, live::Event::MemoryExport { .. })
            })? {
                if bytes.len() != 1 {
                    return Err("One-byte export length mismatch".into());
                }
            }
            // A genuine FAULT must still be reported, then leave reads usable.
            d.request_live(live::Command::Swd("mdw 0xfffffff0 1".into()))?;
            let error = wait(&d, |event| matches!(event, live::Event::Error { .. }))?;
            if !matches!(error, live::Event::Error { message, .. } if message.contains("ACK 0x04"))
            {
                return Err("Expected unmapped-memory FAULT".into());
            }
            d.request_live(live::Command::MemoryRead {
                address: 0x20041FD0,
                length: 1,
                target: "rp2040".into(),
            })?;
            wait(&d, |event| matches!(event, live::Event::SwdMemory { .. }))?;
            eprintln!("One-byte export and read after FAULT passed");
            Ok(())
        })();
        if !initially_halted {
            d.request_live(live::Command::Swd("resume".into())).unwrap();
            wait(&d, |event| {
                matches!(event, live::Event::SwdState { halted: false, .. })
            })
            .unwrap();
        }
        result.unwrap();
    }

    #[test]
    fn console_columns_align_with_multiline_messages() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut d = Device::base("test".into(), "Probe".into());
        d.serial_bytes(238.929, "TX", b"hello".to_vec());
        d.serial_bytes(238.945, "RX", b"hello\nworld".to_vec());
        let output = console_frame(&ctx, &mut d, None, false);
        let mut tx = vec![];
        let mut rx = vec![];
        let mut times = vec![];
        let mut messages = vec![];
        let mut continuation = vec![];
        for shape in &output.shapes {
            text_positions(&shape.shape, "TX", &mut tx);
            text_positions(&shape.shape, "RX", &mut rx);
            text_positions(&shape.shape, "00238.945", &mut times);
            text_positions(&shape.shape, "hello", &mut messages);
            text_positions(&shape.shape, "world", &mut continuation);
        }
        assert!((tx[0].x - rx[0].x).abs() < 0.1);
        assert!((times[0].y - rx[0].y).abs() < 0.1);
        assert!((messages[1].y - rx[0].y).abs() < 0.1);
        assert!((messages[1].x - continuation[0].x).abs() < 0.1);
        assert!(continuation[0].y > messages[1].y);
    }

    #[test]
    fn console_status_shows_uart_and_spi_without_pwm() {
        let ctx = egui::Context::default();
        let mut device = Device::base("test".into(), "Probe".into());
        let output = console_frame(&ctx, &mut device, None, false);
        let mut text = vec![];
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut text);
        }
        let indicators = ctx
            .data(|data| {
                data.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            })
            .unwrap();
        for label in ["Hardware", "UART", "SPI"] {
            assert!(indicators.iter().any(|(name, _)| name == label));
        }
        assert!(!text.iter().any(|text| text == "SPI"));
        assert!(!text.iter().any(|text| text == "Hardware"));
        assert!(!text.iter().any(|text| text == "PWM"));
    }

    #[test]
    fn running_cpu_keeps_cached_special_registers_visible() {
        let ctx = egui::Context::default();
        let mut d = Device::base("test".into(), "Probe".into());
        d.swd = true;
        d.running = true;
        d.registers[15] = 0x10000100;
        d.registers_valid[0] = true;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                live_swd(ui, &mut d, &I18n::default(), &mut String::new())
            });
        });
        let mut text = vec![];
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut text);
        }
        assert!(text.iter().any(|text| text == "PC  10000100"));
    }

    fn text_shapes(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(shape) => text.push(shape.galley.text().into()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text_shapes(shape, text);
                }
            }
            _ => {}
        }
    }

    fn text_positions(shape: &egui::Shape, label: &str, positions: &mut Vec<egui::Pos2>) {
        match shape {
            egui::Shape::Text(shape) if shape.galley.text() == label => positions.push(shape.pos),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text_positions(shape, label, positions);
                }
            }
            _ => {}
        }
    }

    fn transfer_speed_text(ctx: &egui::Context, device: &mut Device, now: f64) -> String {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 400.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| progress(ui, device, &I18n::default(), now));
            },
        );
        let mut text = vec![];
        for shape in &output.shapes {
            text_shapes(&shape.shape, &mut text);
        }
        text.into_iter()
            .find(|text| text.ends_with(" KB/s"))
            .expect("transfer progress must display its speed")
    }

    #[test]
    fn completed_and_failed_transfer_speeds_do_not_include_idle_time() {
        let ctx = egui::Context::default();
        for status in ["Complete", "Failed"] {
            let mut d = Device::base("test".into(), "Probe".into());
            d.task = Some(crate::model::Task {
                kind: TaskKind::Receive,
                started: 10.,
                finished_at: None,
                progress: 1.,
                status: "Running".into(),
                total: 65536,
                transferred: 65536,
            });
            d.task.as_mut().unwrap().finish(status, 14.);
            assert_eq!(transfer_speed_text(&ctx, &mut d, 14.), "16.4 KB/s");
            assert_eq!(transfer_speed_text(&ctx, &mut d, 74.), "16.4 KB/s");
        }
    }

    #[test]
    fn swd_registers_have_no_widget_id_clashes() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        ctx.options_mut(|options| options.warn_on_id_clash = true);
        let mut device = Device::base("test".into(), "Probe".into());
        device.swd_tab = 2;
        let locale = I18n::default();
        for filter in ["", "R", "PC", ""] {
            device.reg_search = filter.into();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 1200.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        live_swd(ui, &mut device, &locale, &mut String::new())
                    });
                },
            );
            let mut text = vec![];
            let mut writes = vec![];
            for shape in &output.shapes {
                text_shapes(&shape.shape, &mut text);
                text_positions(&shape.shape, locale.tr("Write"), &mut writes);
            }
            assert!(!writes.is_empty());
            assert!(
                writes.iter().all(|pos| (pos.x - writes[0].x).abs() < 0.1),
                "Register write buttons must align for R0–R12, SP, LR, PC and xPSR: {writes:?}"
            );
            let warnings: Vec<_> = text
                .iter()
                .filter(|text| text.contains("use of widget ID"))
                .collect();
            assert!(warnings.is_empty(), "egui ID collisions: {warnings:?}");
        }
    }
}
