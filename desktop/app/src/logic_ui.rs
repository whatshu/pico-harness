use crate::theme::{icon_button, icon_menu, icon_toggle, Icon};
use crate::{export, i18n::I18n, live, logic::Logic, model::Device, ACCENT, BORDER, MUTED, RED};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke};
fn pin(ui: &mut egui::Ui, id: &str, value: &mut u8) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("GP{value}"))
        .width(65.)
        .show_ui(ui, |ui| {
            for p in crate::logic::PINS {
                ui.selectable_value(value, p, format!("GP{p}"));
            }
        });
}
pub fn show(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64, notice: &mut String) {
    let available = d.available();
    let session = d.live_session.as_ref().map(|session| session.tx.clone());
    let work_baud = d.baud;
    let s = &mut d.logic;
    let has_capture = !s.samples.is_empty();
    ui.horizontal_wrapped(|ui| {
        if icon_button(
            ui,
            if s.capturing.is_some() { Icon::Stop } else { Icon::Play },
            l.tr(if s.capturing.is_some() { "Stop" } else { "Run" }),
            available
                && (s.capturing.is_some() || s.channels.iter().any(|channel| channel.enabled)),
        )
        .clicked()
        {
            if s.capturing.is_some() {
                s.capturing = None;
                if let Some(tx) = &session {
                    let _ = tx.send(live::Command::CaptureCancel);
                }
            } else {
                s.update_sampling();
                s.capturing = Some(now);
                if let Some(tx) = &session {
                    if tx
                        .send(live::Command::Capture {
                            count: s.count as u32,
                            rate: s.rate,
                            trigger: s.trigger,
                            pin: s.trigger_pin,
                            timeout_ms: s.trigger_timeout_ms,
                        })
                        .is_err()
                    {
                        s.capturing = None;
                        *notice = "USB session closed".into();
                    }
                }
            }
        }
        ui.label(l.tr("Sample rate"));
        egui::ComboBox::from_id_salt("capture-rate")
            .selected_text(crate::logic::frequency_label(s.rate))
            .show_ui(ui, |ui| {
                for rate in crate::logic::SAMPLE_RATES {
                    if ui
                        .selectable_value(&mut s.rate, rate, crate::logic::frequency_label(rate))
                        .changed()
                    {
                        s.update_sampling();
                    }
                }
            });
        ui.label(l.tr("Duration"));
        let mut milliseconds = s.duration_s * 1000.;
        let maximum = crate::logic::MAX_SAMPLES as f64 / s.rate as f64 * 1000.;
        if ui
            .add(
                egui::DragValue::new(&mut milliseconds)
                    .range(if s.trigger == 0 { 1000. } else { 4000. } / s.rate as f64..=maximum)
                    .speed(0.1)
                    .suffix(" ms"),
            )
            .changed()
        {
            s.duration_s = milliseconds / 1000.;
            s.update_sampling();
        }
        ui.label(RichText::new(format!("{} samples · max {maximum:.3} ms", s.count)).color(MUTED));
        ui.menu_button(l.tr("Channels"), |ui| {
            ui.label(RichText::new(l.tr("Target GPIO · Work Core")).small().color(MUTED))
                .on_hover_text(l.tr("Only GPIO connected to the carrier can be sampled. GP25 (onboard LED) is not connected."));
            ui.separator();
            for (label, selection) in [
                ("Select all", Some(true)),
                ("Select none", Some(false)),
                ("PicoHarness defaults", None),
            ] {
                if ui.button(l.tr(label)).clicked() {
                    s.select_channels(selection);
                    if selection.is_none() {
                        for decoder in &mut s.decoders {
                            if let crate::logic::Decoder::Uart { baud, .. } = &mut decoder.decoder {
                                *baud = work_baud;
                            }
                        }
                        s.decode();
                    }
                }
            }
            ui.separator();
            for channel in &mut s.channels {
                let label = if channel.name.is_empty() {
                    format!("GP{}", channel.pin)
                } else {
                    format!("GP{} · {}", channel.pin, channel.name)
                };
                ui.checkbox(&mut channel.enabled, label);
            }
        });
        if icon_button(ui, Icon::ZoomIn, l.tr("Zoom in"), has_capture).clicked()
        {
            s.span = (s.span * 0.5).max(10. / s.sample_rate as f64);
        }
        if icon_button(ui, Icon::ZoomOut, l.tr("Zoom out"), has_capture).clicked()
        {
            s.span = (s.span * 2.).min(s.duration());
        }
        if icon_button(ui, Icon::Fit, l.tr("Fit"), has_capture).clicked()
        {
            s.offset = 0.;
            s.span = s.duration();
        }
        icon_menu(ui, Icon::Decoder, l.tr("Add decoder"), |ui| {
            for decoder in crate::logic::Decoder::catalog() {
                if ui.button(decoder.name()).clicked() {
                    s.add_decoder(decoder);
                    ui.close_menu();
                }
            }
        });
        if icon_toggle(ui, Icon::Cursors, l.tr("Cursors"), s.cursors, true).clicked() {
            s.cursors = !s.cursors;
        }
        ui.add_enabled_ui(has_capture, |ui| {
            icon_menu(ui, Icon::Export, l.tr("Export"), |ui| {
                for label in ["CSV", "VCD", "Decode CSV"] {
                    if ui.button(label).clicked() {
                        let (name, data) = match label {
                            "VCD" => ("capture.vcd", s.vcd()),
                            "CSV" => ("capture.csv", s.csv()),
                            _ => ("decode.csv", s.annotation_csv()),
                        };
                        if let Err(e) = export(name, data) {
                            *notice = e;
                        }
                        ui.close_menu();
                    }
                }
            })
        });
    });
    crate::theme::row(ui, |ui| {
        ui.label(l.tr("Trigger"));
        egui::ComboBox::from_id_salt("trigger-edge")
            .selected_text(l.tr(["None", "Rising edge", "Falling edge"][s.trigger as usize]))
            .show_ui(ui, |ui| {
                for (edge, label) in ["None", "Rising edge", "Falling edge"]
                    .into_iter()
                    .enumerate()
                {
                    ui.selectable_value(&mut s.trigger, edge as u8, l.tr(label));
                }
            });
        ui.add_enabled_ui(s.trigger != 0, |ui| {
            pin(ui, "trigger-pin", &mut s.trigger_pin);
            ui.add(
                egui::DragValue::new(&mut s.trigger_timeout_ms)
                    .range(1..=10000)
                    .suffix(" ms timeout"),
            );
        });
        ui.label(
            RichText::new(l.tr(if s.capturing.is_some() && s.trigger != 0 {
                "Waiting for edge…"
            } else {
                "Click a channel label to configure it"
            }))
            .small()
            .color(MUTED),
        );
    });
    ui.separator();
    if s.cursors && has_capture {
        ui.horizontal(|ui| {
            ui.colored_label(ACCENT, format!("A {:.3} ms", s.a * 1000.));
            ui.colored_label(Color32::LIGHT_BLUE, format!("B {:.3} ms", s.b * 1000.));
            let delta = (s.b - s.a).abs();
            ui.monospace(format!(
                "Δt {:.3} µs    {:.3} kHz    {} samples",
                delta * 1e6,
                if delta > 0. { 1. / delta / 1000. } else { 0. },
                (delta * s.sample_rate as f64).round()
            ));
        });
    }
    let height = (ui.available_height() - 165.).max(140.);
    egui::ScrollArea::vertical()
        .id_salt("trace-scroll")
        .max_height(height)
        .min_scrolled_height(height)
        .show(ui, |ui| {
            waveform(ui, s, l, height);
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new(l.tr("Decoded events")).strong());
        ui.label(RichText::new(format!("{}", s.annotations.len())).color(MUTED));
        ui.label(
            RichText::new(l.tr("Click an event to locate it in the waveform"))
                .small()
                .color(MUTED),
        );
    });
    egui::ScrollArea::vertical()
        .id_salt("decode-results")
        .max_height(125.)
        .show_rows(
            ui,
            ui.spacing()
                .interact_size
                .y
                .max(ui.text_style_height(&egui::TextStyle::Body)),
            s.annotations.len(),
            |ui, visible| {
                egui::Grid::new("decode-table")
                    .striped(true)
                    .start_row(visible.start)
                    .min_row_height(ui.spacing().interact_size.y)
                    .num_columns(4)
                    .spacing([25., 4.])
                    .show(ui, |ui| {
                        for i in visible {
                            let a = &s.annotations[i];
                            let selected = s.selected == Some(i);
                            if ui
                                .selectable_label(selected, format!("{:.3} ms", a.start * 1000.))
                                .clicked()
                            {
                                s.selected = Some(i);
                                s.span = ((a.end - a.start) * 5.).max(50. / s.sample_rate as f64);
                                s.offset = (a.start - s.span * 0.2).max(0.);
                            }
                            ui.colored_label(
                                if a.protocol == "UART" {
                                    ACCENT
                                } else {
                                    Color32::LIGHT_BLUE
                                },
                                &a.protocol,
                            );
                            ui.monospace(&a.text);
                            ui.colored_label(
                                if a.error { RED } else { MUTED },
                                if a.error { l.tr("Framing error") } else { "OK" },
                            );
                            ui.end_row();
                        }
                    });
            },
        );
    if d.live_error.starts_with("Logic:") {
        ui.colored_label(RED, &d.live_error);
    }
}
fn waveform(ui: &mut egui::Ui, s: &mut Logic, l: &I18n, height: f32) {
    let label_w = 175.;
    let rows = s.channels.iter().filter(|c| c.enabled).count() + s.decoders.len();
    let row_h = ((height - 38.0) / rows.max(1) as f32).clamp(28.0, 43.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 38. + rows as f32 * row_h),
        egui::Sense::click_and_drag(),
    );
    let trace = Rect::from_min_max(egui::pos2(rect.left() + label_w, rect.top()), rect.max);
    let duration = s.duration();
    let has_capture = !s.samples.is_empty();
    if has_capture {
        s.span = s.span.clamp(
            10. / s.sample_rate as f64,
            duration.max(10. / s.sample_rate as f64),
        );
    }
    s.offset = s.offset.clamp(0., (duration - s.span).max(0.));
    if response.hovered() && has_capture {
        let delta = ui.input(|i| i.smooth_scroll_delta.y);
        if delta.abs() > 0.01 {
            if let Some(pos) = response.hover_pos() {
                let fraction = ((pos.x - trace.left()) / trace.width()).clamp(0., 1.) as f64;
                let t = s.offset + fraction * s.span;
                s.span = (s.span * (-delta as f64 * 0.005).exp())
                    .clamp(10. / s.sample_rate as f64, duration);
                s.offset = (t - fraction * s.span).max(0.);
            }
        }
    }
    let x = |t: f64| trace.left() + ((t - s.offset) / s.span) as f32 * trace.width();
    let mut cursor_drag = false;
    if let Some(pos) = response.interact_pointer_pos() {
        if s.cursors && response.dragged() && has_capture {
            let origin = pos - response.drag_delta();
            let ax = x(s.a);
            let bx = x(s.b);
            let time = s.offset + ((pos.x - trace.left()) / trace.width()) as f64 * s.span;
            if (origin.x - ax).abs() < 12. {
                s.a = time.clamp(0., duration);
                cursor_drag = true;
            } else if (origin.x - bx).abs() < 12. {
                s.b = time.clamp(0., duration);
                cursor_drag = true;
            }
        }
        if response.double_clicked() && has_capture && pos.y < rect.top() + 38. {
            let time = s.offset + ((pos.x - trace.left()) / trace.width()) as f64 * s.span;
            s.markers.push((time, format!("M{}", s.markers.len() + 1)));
        }
    }
    if response.dragged() && !cursor_drag {
        s.offset = (s.offset
            - ui.input(|i| i.pointer.delta().x) as f64 / trace.width() as f64 * s.span)
            .clamp(0., (duration - s.span).max(0.));
    }
    let (offset, span) = (s.offset, s.span);
    let x = |t: f64| trace.left() + ((t - offset) / span) as f32 * trace.width();
    let painter = ui.painter_at(trace);
    painter.rect_filled(trace, 0, ui.visuals().extreme_bg_color);
    if let Some(index) = s.trigger_index {
        let xx = x(index as f64 / s.sample_rate as f64);
        painter.line_segment(
            [egui::pos2(xx, trace.top()), egui::pos2(xx, trace.bottom())],
            Stroke::new(1.0_f32, Color32::from_rgb(208, 178, 118)),
        );
        painter.text(
            egui::pos2(xx + 4., trace.top() + 4.),
            egui::Align2::LEFT_TOP,
            "T",
            egui::FontId::monospace(11.),
            Color32::from_rgb(208, 178, 118),
        );
    }
    if !has_capture {
        painter.text(
            trace.center(),
            egui::Align2::CENTER_CENTER,
            l.tr(if s.capturing.is_some() {
                "Capturing…"
            } else {
                "No capture yet"
            }),
            egui::FontId::proportional(13.),
            MUTED,
        );
    }
    for n in 0..=10 {
        let xx = trace.left() + trace.width() * n as f32 / 10.;
        painter.line_segment(
            [
                egui::pos2(xx, trace.top() + 30.),
                egui::pos2(xx, trace.bottom()),
            ],
            Stroke::new(1.0_f32, BORDER),
        );
        let time = s.offset + s.span * n as f64 / 10.;
        if has_capture {
            painter.text(
                egui::pos2(xx + 3., trace.top() + 10.),
                egui::Align2::LEFT_CENTER,
                format!("{:.3} ms", time * 1000.),
                egui::FontId::monospace(11.),
                MUTED,
            );
        }
    }
    let mut row = 0usize;
    let mut channel_decoder = None;
    for c in &mut s.channels {
        if !c.enabled {
            continue;
        }
        let top = rect.top() + 38. + row as f32 * row_h;
        row += 1;
        let color = [
            ACCENT,
            Color32::from_rgb(143, 182, 157),
            Color32::from_rgb(208, 178, 118),
        ][row % 3];
        let label = Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(label_w - 5., row_h),
        );
        if !ui.is_rect_visible(Rect::from_min_size(
            label.min,
            egui::vec2(rect.width(), row_h),
        )) {
            continue;
        }
        let r = ui.interact(
            label,
            ui.id().with(("channel", c.pin)),
            egui::Sense::click(),
        );
        r.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("GP{}", c.pin),
            )
        });
        if r.hovered() {
            ui.painter()
                .rect_filled(label, 0, ui.visuals().widgets.hovered.bg_fill);
        }
        let arrow = if s.trigger_pin == c.pin {
            ["", " ↑", " ↓"][s.trigger as usize]
        } else {
            ""
        };
        ui.painter().with_clip_rect(label).text(
            label.left_center() + egui::vec2(6., if c.name.is_empty() { 0. } else { -7. }),
            egui::Align2::LEFT_CENTER,
            format!("GP{}{arrow}", c.pin),
            egui::FontId::monospace(12.),
            color,
        );
        ui.painter().with_clip_rect(label).text(
            label.left_center() + egui::vec2(6., 10.),
            egui::Align2::LEFT_CENTER,
            &c.name,
            egui::FontId::proportional(10.),
            MUTED,
        );
        let popup_id = r.id.with("configuration");
        if r.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(popup_id));
        }
        egui::popup::popup_below_widget(
            ui,
            popup_id,
            &r,
            egui::popup::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_width(196.);
                crate::theme::row(ui, |ui| {
                    ui.label(RichText::new(format!("GP{}", c.pin)).monospace().strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_toggle(ui, Icon::Eye, l.tr("Enabled"), c.enabled, true).clicked() {
                            c.enabled = !c.enabled;
                        }
                    });
                });
                let name = ui.add(
                    egui::TextEdit::singleline(&mut c.name)
                        .hint_text(l.tr("Channel name"))
                        .desired_width(196.)
                        .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT)),
                );
                crate::theme::accessible_label(&name, l.tr("Channel name"));
                ui.separator();
                crate::theme::row(ui, |ui| {
                    for (edge, icon, label) in [
                        (0, Icon::NoTrigger, "No trigger"),
                        (1, Icon::Rising, "Trigger on rising edge"),
                        (2, Icon::Falling, "Trigger on falling edge"),
                    ] {
                        if icon_toggle(
                            ui,
                            icon,
                            l.tr(label),
                            s.trigger == edge && (edge == 0 || s.trigger_pin == c.pin),
                            true,
                        )
                        .clicked()
                        {
                            s.trigger = edge;
                            s.trigger_pin = c.pin;
                            ui.memory_mut(|memory| memory.close_popup());
                        }
                    }
                    ui.separator();
                    icon_menu(ui, Icon::Decoder, l.tr("Add channel decoder"), |ui| {
                        for mut decoder in crate::logic::Decoder::catalog() {
                            if ui.button(decoder.name()).clicked() {
                                match &mut decoder {
                                    crate::logic::Decoder::Uart { pin, .. }
                                    | crate::logic::Decoder::Pwm { pin } => *pin = c.pin,
                                    _ => {}
                                }
                                channel_decoder = Some(decoder);
                                ui.memory_mut(|memory| memory.close_popup());
                            }
                        }
                    });
                });
            },
        );
        let start = (s.offset * s.sample_rate as f64) as usize;
        let end = (((s.offset + s.span) * s.sample_rate as f64) as usize + 1).min(s.samples.len());
        if start < end {
            let high = top + 6.;
            let low = top + row_h - 8.;
            let steps = digital_steps(
                &s.samples[start..end],
                c.pin,
                (trace.width() * ui.ctx().pixels_per_point()).ceil() as usize,
            );
            let last = s.samples[end - 1] & (1 << c.pin) != 0;
            let mut points: Vec<Pos2> = steps
                .into_iter()
                .map(|(index, value)| {
                    egui::pos2(
                        x((start + index) as f64 / s.sample_rate as f64),
                        if value { high } else { low },
                    )
                })
                .collect();
            points.push(egui::pos2(trace.right(), if last { high } else { low }));
            painter.add(egui::Shape::line(points, Stroke::new(1.4_f32, color)));
        }
        painter.line_segment(
            [
                egui::pos2(trace.left(), top + row_h),
                egui::pos2(trace.right(), top + row_h),
            ],
            Stroke::new(0.5_f32, BORDER),
        );
    }
    if let Some(decoder) = channel_decoder {
        s.add_decoder(decoder);
    }
    let before = s.decoders.clone();
    let mut remove_decoder = None;
    for instance in &mut s.decoders {
        let protocol = &instance.name;
        let color = if !instance.enabled {
            MUTED
        } else if matches!(instance.decoder, crate::logic::Decoder::Uart { .. }) {
            ACCENT
        } else {
            Color32::LIGHT_BLUE
        };
        let top = rect.top() + 38. + row as f32 * row_h;
        row += 1;
        if !ui.is_rect_visible(Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), row_h),
        )) {
            continue;
        }
        let header = Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(label_w - 5., row_h),
        );
        ui.painter().with_clip_rect(header).text(
            header.left_center() + egui::vec2(6., 0.),
            egui::Align2::LEFT_CENTER,
            format!("{protocol} · decode"),
            egui::FontId::monospace(12.),
            color,
        );
        let response = ui.interact(
            header,
            ui.id().with(("decoder", instance.id)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                format!("{} · decode", instance.name),
            )
        });
        let popup_id = response.id.with("configuration");
        if response.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(popup_id));
        }
        egui::popup::popup_below_widget(
            ui,
            popup_id,
            &response,
            egui::popup::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_width(240.);
                decoder_controls(ui, instance, l);
                if icon_button(ui, Icon::Trash, l.tr("Remove"), true).clicked() {
                    remove_decoder = Some(instance.id);
                    ui.memory_mut(|memory| memory.close_popup());
                }
            },
        );
        let mut last_column = None;
        for a in s.annotations.iter().filter(|a| a.decoder_id == instance.id) {
            if a.end < s.offset || a.start > s.offset + s.span {
                continue;
            }
            let column = x(a.start).floor() as i32;
            let subpixel = x(a.end) - x(a.start) < 1.;
            if subpixel && last_column == Some(column) {
                continue;
            }
            last_column = if subpixel { Some(column) } else { None };
            let r = Rect::from_min_max(
                egui::pos2(x(a.start), top + 5.),
                egui::pos2(x(a.end).max(x(a.start) + 3.), top + row_h - 5.),
            );
            let color = if a.error { RED } else { color };
            let inset = (r.width() / 2.).min(5.);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(r.left(), r.center().y),
                    egui::pos2(r.left() + inset, r.top()),
                    egui::pos2(r.right() - inset, r.top()),
                    egui::pos2(r.right(), r.center().y),
                    egui::pos2(r.right() - inset, r.bottom()),
                    egui::pos2(r.left() + inset, r.bottom()),
                ],
                color.gamma_multiply(0.18),
                Stroke::new(1.0_f32, color),
            ));
            if r.width() > 32. {
                painter.with_clip_rect(r.intersect(trace)).text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    &a.text,
                    egui::FontId::monospace(11.),
                    color,
                );
            }
        }
    }
    if let Some(id) = remove_decoder {
        s.decoders.retain(|decoder| decoder.id != id);
    }
    if before != s.decoders {
        s.enable_decoder_channels();
        s.decode();
    }
    if s.cursors && has_capture {
        for (t, name, color) in [(s.a, "A", ACCENT), (s.b, "B", Color32::LIGHT_BLUE)] {
            let xx = x(t);
            painter.line_segment(
                [
                    egui::pos2(xx, trace.top() + 25.),
                    egui::pos2(xx, trace.bottom()),
                ],
                Stroke::new(1.3_f32, color),
            );
            painter.rect_filled(
                Rect::from_center_size(egui::pos2(xx, trace.top() + 20.), egui::vec2(18., 18.)),
                3,
                color,
            );
            painter.text(
                egui::pos2(xx, trace.top() + 20.),
                egui::Align2::CENTER_CENTER,
                name,
                egui::FontId::monospace(12.),
                Color32::BLACK,
            );
        }
    }
    for (t, name) in &s.markers {
        let xx = x(*t);
        painter.line_segment(
            [
                egui::pos2(xx, trace.top() + 28.),
                egui::pos2(xx, trace.bottom()),
            ],
            Stroke::new(1.0_f32, Color32::YELLOW),
        );
        painter.text(
            egui::pos2(xx + 3., trace.top() + 30.),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::monospace(10.),
            Color32::YELLOW,
        );
    }
}

// At overview zoom, retain both levels and the final state within each pixel bucket.
// At sample zoom, every transition keeps its exact sample index. Capture/export data
// and protocol decoding always use the original samples.
fn digital_steps(samples: &[u32], pin: u8, columns: usize) -> Vec<(usize, bool)> {
    let Some(first) = samples.first() else {
        return vec![];
    };
    let level = |value: u32| value & (1 << pin) != 0;
    let bucket = samples.len().div_ceil(columns.max(1));
    let mut last = level(*first);
    let mut points = vec![(0, last)];
    for (index, chunk) in samples.chunks(bucket).enumerate() {
        let offset = index * bucket;
        let initial = level(chunk[0]);
        if initial != last {
            points.extend([(offset, last), (offset, initial)]);
            last = initial;
        }
        if let Some(transition) = chunk.iter().position(|value| level(*value) != initial) {
            points.extend([
                (offset + transition, initial),
                (offset + transition, !initial),
            ]);
            last = !initial;
        }
        let final_level = level(*chunk.last().unwrap());
        if final_level != last {
            let end = offset + chunk.len() - 1;
            points.extend([(end, last), (end, final_level)]);
            last = final_level;
        }
    }
    points
}

fn decoder_controls(ui: &mut egui::Ui, instance: &mut crate::logic::DecoderInstance, l: &I18n) {
    ui.checkbox(&mut instance.enabled, l.tr("Enabled"));
    ui.label(l.tr("Channel name"));
    crate::theme::text_field_sized(ui, &mut instance.name, 180.);
    ui.separator();
    egui::Grid::new(("decoder-parameters", instance.id))
        .num_columns(2)
        .spacing([12., 5.])
        .show(ui, |ui| {
            use crate::logic::Decoder;
            match &mut instance.decoder {
                Decoder::Uart {
                    pin: channel,
                    baud,
                    bits,
                    parity,
                    stop,
                } => {
                    ui.label("RX");
                    pin(ui, "rx", channel);
                    ui.end_row();
                    ui.label(l.tr("Baud"));
                    ui.add(egui::DragValue::new(baud).range(300..=2_000_000));
                    ui.end_row();
                    ui.label(l.tr("Data bits"));
                    ui.add(egui::DragValue::new(bits).range(5..=8));
                    ui.end_row();
                    ui.label(l.tr("Parity"));
                    egui::ComboBox::from_id_salt("parity")
                        .selected_text(l.tr(["None", "Even", "Odd"][*parity as usize]))
                        .show_ui(ui, |ui| {
                            for (i, label) in ["None", "Even", "Odd"].iter().enumerate() {
                                ui.selectable_value(parity, i as u8, l.tr(label));
                            }
                        });
                    ui.end_row();
                    ui.label(l.tr("Stop bits"));
                    ui.add(egui::DragValue::new(stop).range(1..=2));
                    ui.end_row();
                }
                Decoder::Spi {
                    pins,
                    mode,
                    lsb,
                    bits,
                } => {
                    for (label, channel) in ["CLK", "MOSI", "MISO", "CS"].into_iter().zip(pins) {
                        ui.label(label);
                        pin(ui, label, channel);
                        ui.end_row();
                    }
                    ui.label(l.tr("Mode"));
                    ui.add(egui::DragValue::new(mode).range(0..=3));
                    ui.end_row();
                    ui.label(l.tr("Bit order"));
                    ui.checkbox(lsb, "LSB first");
                    ui.end_row();
                    ui.label(l.tr("Word bits"));
                    ui.add(egui::DragValue::new(bits).range(4..=32));
                    ui.end_row();
                }
                Decoder::I2c { scl, sda } => {
                    ui.label("SCL");
                    pin(ui, "scl", scl);
                    ui.end_row();
                    ui.label("SDA");
                    pin(ui, "sda", sda);
                    ui.end_row();
                }
                Decoder::Pwm { pin: channel } => {
                    ui.label(l.tr("Signal"));
                    pin(ui, "signal", channel);
                    ui.end_row();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_geometry_is_bounded_and_preserves_one_sample_pulses() {
        let mut samples = vec![0; 32768];
        samples[117] = 1 << 5;
        samples[20117] = 1 << 5;
        let points = digital_steps(&samples, 5, 100);
        assert!(points.contains(&(117, true)));
        assert!(points.contains(&(20117, true)));
        assert!(!points.last().unwrap().1);
        let dense: Vec<_> = (0..32768).map(|i| (i % 2) << 5).collect();
        let points = digital_steps(&dense, 5, 100);
        assert!(points.len() <= 6 * 100 + 1);
        assert_eq!(
            points.last().unwrap().1,
            dense.last().unwrap() & (1 << 5) != 0
        );
    }
    #[test]
    fn sample_zoom_preserves_every_transition_exactly() {
        assert_eq!(
            digital_steps(&[0, 0, 32, 32, 0], 5, 100),
            vec![(0, false), (2, false), (2, true), (4, true), (4, false)]
        );
        assert!(digital_steps(&[], 5, 0).is_empty());
    }
}
