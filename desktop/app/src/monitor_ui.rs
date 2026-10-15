use crate::{export, file_pick, i18n::I18n, model::Device, BORDER, MUTED, RED};
use eframe::egui::{self, Color32, RichText, Stroke};
use picoharness_desktop::telemetry::{self, ChartViewport, Sample, MAX_SAMPLE_GAP};

const COLORS: [Color32; 3] = [
    Color32::from_rgb(215, 179, 95),
    Color32::from_rgb(121, 178, 150),
    Color32::from_rgb(129, 174, 210),
];
const NAMES: [&str; 3] = ["Power", "Voltage", "Current"];
const UNITS: [&str; 3] = ["mW", "V", "mA"];

pub fn show(ui: &mut egui::Ui, d: &mut Device, l: &I18n, now: f64, notice: &mut String) {
    ui.vertical(|ui| {
        controls(ui, d, l, notice);
        ui.add_space(5.);
        let samples = d.monitor.samples(now);
        let latest = if d.monitor.history_view || d.monitor.paused() {
            samples.last().copied()
        } else if d.measurement_fresh(now) {
            d.monitor.live.back()
        } else {
            None
        }
        .cloned();
        let mut viewport = d.monitor.viewport;
        let time = viewport.time.unwrap_or_else(|| {
            time_range(
                &samples,
                d.monitor.history_view,
                d.monitor.display_time(now),
                d.monitor.window_s,
            )
        });
        // Wrapping and expanded events give space back to the charts after a resize.
        let footer_id = ui.make_persistent_id(("monitor-footer-height", &d.id));
        let footer_height = ui
            .data(|data| data.get_temp::<f32>(footer_id))
            .unwrap_or(122.);
        let height = (ui.available_height() - footer_height - 28.).max(150.);
        let mut selected = d.monitor.metric;
        channel_view(
            ui,
            &samples,
            latest.as_ref(),
            ChannelView {
                time,
                height,
                history: d.monitor.history_view,
                zero_baseline: d.monitor.zero_baseline,
                viewport: &mut viewport,
            },
            &mut selected,
            l,
        );
        let footer_top = ui.cursor().top();
        ui.add_space(5.);
        let stats = telemetry::stats(&samples, selected);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(l.tr(NAMES[selected])).color(COLORS[selected]));
            ui.separator();
            if let Some(stats) = stats {
                for (name, value) in [
                    ("Minimum", stats.min),
                    ("Average", stats.mean),
                    ("Maximum", stats.max),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(l.tr(name)).small().color(MUTED));
                        ui.label(
                            RichText::new(format!("{value:.3} {}", UNITS[selected])).monospace(),
                        );
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(l.tr("Energy in view")).small().color(MUTED));
                    ui.label(RichText::new(format!("{:.4} mWh", stats.energy_mwh)).monospace());
                });
            } else {
                ui.label(RichText::new(l.tr("No samples yet")).color(MUTED));
            }
        });
        drop(samples);
        d.monitor.viewport = viewport;
        d.monitor.metric = selected;
        ui.separator();
        let recorded = if d.monitor.history_view || d.monitor.paused() {
            latest.as_ref()
        } else {
            None
        };
        let voltage =
            recorded
                .map(|s| s.work_3v3_v)
                .or(if d.live_ready { Some(d.voltage) } else { None });
        let power = recorded.map_or(d.power, |s| s.power_on);
        let fault = recorded.map_or(d.fault, |s| s.power_fault);
        crate::theme::row(ui, |ui| {
            ui.label(
                RichText::new(l.tr(if d.monitor.history_view {
                    "Recorded status"
                } else {
                    "Device health"
                }))
                .small()
                .color(MUTED),
            );
            ui.separator();
            ui.label(l.tr("Work power"));
            ui.label(l.tr(if power { "Power on" } else { "Power off" }));
            ui.separator();
            ui.label("Work 3V3");
            ui.label(
                RichText::new(
                    voltage
                        .map(|v| format!("{v:.3} V"))
                        .unwrap_or_else(|| "— V".into()),
                )
                .monospace(),
            )
            .on_hover_text(
                l.tr("Work 3V3 is a separate ADC reading; the charts show the INA supply branch."),
            );
            ui.separator();
            ui.label(l.tr("Power fault"));
            ui.colored_label(
                if fault { RED } else { MUTED },
                l.tr(if fault { "Yes" } else { "No" }),
            );
            if ui.available_width() >= 130. {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(l.tr(if d.monitor.history_view {
                            "Saved history"
                        } else if d.monitor.paused() {
                            "Refresh paused · recording continues"
                        } else if d.measurement_fresh(now) {
                            "Live · 2 Hz"
                        } else {
                            "Waiting for measurements"
                        }))
                        .small()
                        .color(MUTED),
                    );
                });
            }
        });
        ui.collapsing(l.tr("Recent events"), |ui| {
            if d.events.is_empty() {
                ui.label(RichText::new(l.tr("No events yet")).color(MUTED));
            }
            for event in d.events.iter().rev().take(10) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{:02}:{:02}",
                            event.time as u64 / 60,
                            event.time as u64 % 60
                        ))
                        .monospace()
                        .color(MUTED),
                    );
                    ui.label(l.tr(&event.text));
                });
            }
        });
        if !d.live_error.is_empty() {
            ui.colored_label(RED, &d.live_error);
        }
        if !d.monitor.error.is_empty() {
            ui.colored_label(RED, l.tr(&d.monitor.error));
        }
        let footer_height = ui.cursor().top() - footer_top;
        ui.data_mut(|data| data.insert_temp(footer_id, footer_height));
    });
}

fn controls(ui: &mut egui::Ui, d: &mut Device, l: &I18n, notice: &mut String) -> Vec<egui::Rect> {
    use crate::theme::{button, toggle};
    let mut rects = Vec::new();
    let wide = ui.available_width() >= 900.;
    crate::theme::row(ui, |ui| {
        let live = toggle(
            ui,
            l.tr("Live"),
            52.,
            !d.monitor.history_view,
            d.online || !d.monitor.live.is_empty(),
        );
        rects.push(live.rect);
        if live.clicked() {
            d.monitor.history_view = false;
            d.monitor.viewport = ChartViewport::default();
        }
        let history = toggle(
            ui,
            l.tr("History"),
            60.,
            d.monitor.history_view,
            d.monitor.history().is_some(),
        );
        rects.push(history.rect);
        if history.clicked() {
            d.monitor.history_view = true;
            d.monitor.viewport = ChartViewport::default();
        }
        let pause = crate::theme::refresh_button(
            ui,
            d.monitor.paused(),
            !d.monitor.history_view && !d.monitor.live.is_empty(),
        );
        rects.push(pause.rect);
        if pause.clicked() {
            d.monitor.set_paused(!d.monitor.paused());
            if !d.monitor.paused() {
                d.monitor.viewport = ChartViewport::default();
            }
        }
        pause.on_hover_text(format!(
            "{}\n{}",
            l.tr(if d.monitor.paused() {
                "Resume refresh"
            } else {
                "Pause refresh"
            }),
            l.tr("Freeze this view; measurement and recording continue.")
        ));
        let fit = crate::theme::icon_button(ui, crate::theme::Icon::Fit, l.tr("Fit charts"), true);
        rects.push(fit.rect);
        if fit.clicked() {
            d.monitor.viewport = ChartViewport::default();
        }
        ui.separator();
        let recording = d.monitor.recording();
        let record = button(
            ui,
            l.tr(if recording {
                "Stop recording"
            } else {
                "Record"
            }),
            92.,
            recording || d.live_ready,
        );
        rects.push(record.rect);
        if record.clicked() {
            let result = if recording {
                d.monitor.stop()
            } else {
                d.monitor.start(&d.id, &telemetry::recording_dir())
            };
            if let Err(e) = result {
                *notice = e;
            }
        }
        if wide {
            rects.extend(file_controls(ui, d, l, notice));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let zero = ui.checkbox(&mut d.monitor.zero_baseline, l.tr("Zero baseline"));
            if zero.changed() {
                d.monitor.viewport.values = [None; 3];
            }
            rects.push(zero.rect);
            let range = egui::ComboBox::from_id_salt("monitor-window")
                .width(95.)
                .selected_text(if d.monitor.history_view {
                    l.tr("Full session").into()
                } else {
                    format!(
                        "{} {}",
                        l.tr("Range"),
                        if d.monitor.window_s == 30. {
                            "30 s"
                        } else if d.monitor.window_s == 60. {
                            "60 s"
                        } else if d.monitor.window_s == 300. {
                            "5 min"
                        } else {
                            "1 h"
                        }
                    )
                })
                .show_ui(ui, |ui| {
                    if d.monitor.history_view {
                        ui.label(l.tr("Full session"));
                    } else {
                        for (seconds, label) in [
                            (30., "30 s"),
                            (60., "60 s"),
                            (300., "5 min"),
                            (3600., "1 h"),
                        ] {
                            if ui
                                .selectable_value(&mut d.monitor.window_s, seconds, label)
                                .changed()
                            {
                                d.monitor.viewport = ChartViewport::default();
                            }
                        }
                    }
                });
            rects.push(range.response.rect);
        });
    });
    if !wide {
        crate::theme::row(ui, |ui| rects.extend(file_controls(ui, d, l, notice)));
    }
    let mut current = false;
    if let Some(h) = d.monitor.history() {
        ui.horizontal(|ui| {
            if d.monitor.recording() {
                ui.label(RichText::new("REC").color(RED).small());
            }
            let file = h
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| h.serial.clone());
            ui.label(
                RichText::new(format!(
                    "{file}   ·   {} {}   ·   {:.1} s",
                    h.samples.len(),
                    l.tr("samples"),
                    h.duration()
                ))
                .small()
                .color(MUTED),
            )
            .on_hover_text(
                h.path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            );
            if d.monitor.replay.is_some()
                && d.monitor.session.is_some()
                && ui.small_button(l.tr("Current recording")).clicked()
            {
                current = true;
            }
        });
    }
    if current {
        d.monitor.replay = None;
    }
    rects
}

fn file_controls(
    ui: &mut egui::Ui,
    d: &mut Device,
    l: &I18n,
    notice: &mut String,
) -> Vec<egui::Rect> {
    use crate::theme::button;
    let mut rects = Vec::new();
    let open = button(ui, l.tr("Open history"), 100., true);
    rects.push(open.rect);
    if open.clicked() {
        if let Some(path) = file_pick(&["jsonl"]) {
            if let Err(e) = d.monitor.load(std::path::Path::new(&path)) {
                *notice = e;
            }
        }
    }
    let saved = egui::ComboBox::from_id_salt("saved-recordings")
        .width(112.)
        .selected_text(l.tr("Saved recordings"))
        .show_ui(ui, |ui| {
            let mut paths: Vec<_> = std::fs::read_dir(telemetry::recording_dir())
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                .collect();
            paths.sort();
            if paths.is_empty() {
                ui.label(l.tr("No saved recordings"));
            }
            for path in paths.iter().rev().take(100) {
                if ui
                    .selectable_label(
                        false,
                        path.file_name().unwrap_or_default().to_string_lossy(),
                    )
                    .clicked()
                {
                    if let Err(e) = d.monitor.load(path) {
                        *notice = e;
                    }
                }
            }
        });
    rects.push(saved.response.rect);
    let export_button = button(
        ui,
        l.tr("Export CSV"),
        86.,
        d.monitor.history().is_some_and(|h| !h.samples.is_empty()),
    );
    rects.push(export_button.rect);
    if export_button.clicked() {
        if let Some(h) = d.monitor.history() {
            if let Err(e) = export("pico-power-history.csv", h.csv()) {
                *notice = e;
            }
        }
    }
    rects
}

fn time_range(samples: &[&Sample], history: bool, now: f64, window: f64) -> (f64, f64) {
    if history {
        let start = samples.first().map_or(0., |s| s.elapsed_s);
        let end = samples
            .last()
            .map_or(start + 1., |s| s.elapsed_s)
            .max(start + 1.);
        (start, end)
    } else {
        (now - window, now)
    }
}
fn value_range(samples: &[&Sample], metric: usize, zero_baseline: bool) -> (f64, f64) {
    let Some(stats) = telemetry::stats(samples, metric) else {
        return (0., 1.);
    };
    let padding = ((stats.max - stats.min) * 0.15)
        .max(stats.max.abs() * 0.0001)
        .max(0.01);
    let low = stats.min - padding;
    let high = stats.max + padding;
    if zero_baseline {
        (low.min(0.), high.max(0.))
    } else {
        (low, high)
    }
}
fn position(
    rect: egui::Rect,
    sample: &Sample,
    metric: usize,
    time: (f64, f64),
    range: (f64, f64),
) -> egui::Pos2 {
    egui::pos2(
        rect.left() + ((sample.elapsed_s - time.0) / (time.1 - time.0)) as f32 * rect.width(),
        rect.bottom()
            - ((sample.value(metric) - range.0) / (range.1 - range.0)) as f32 * rect.height(),
    )
}
fn draw_trace(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &[&Sample],
    metric: usize,
    color: Color32,
    time: (f64, f64),
    range: (f64, f64),
) {
    let painter = painter.with_clip_rect(rect);
    for segment in trace_segments(samples, metric, time, rect.width()) {
        for pair in segment.windows(2) {
            painter.line_segment(
                [
                    position(rect, pair[0], metric, time, range),
                    position(rect, pair[1], metric, time, range),
                ],
                Stroke::new(1.2_f32, color),
            );
        }
    }
}

// Preserve spikes and real acquisition gaps while bounding the amount of geometry.
fn trace_segments<'a>(
    samples: &[&'a Sample],
    metric: usize,
    time: (f64, f64),
    pixels: f32,
) -> Vec<Vec<&'a Sample>> {
    samples
        .chunk_by(|a, b| b.elapsed_s - a.elapsed_s <= MAX_SAMPLE_GAP)
        .map(|segment| {
            let span = segment.last().unwrap().elapsed_s - segment[0].elapsed_s;
            let budget = ((span / (time.1 - time.0) * pixels as f64).ceil() as usize).max(2);
            let chunk_size = segment.len().div_ceil(budget);
            let mut points = vec![segment[0]];
            for chunk in segment.chunks(chunk_size) {
                let min = *chunk
                    .iter()
                    .min_by(|a, b| a.value(metric).total_cmp(&b.value(metric)))
                    .unwrap();
                let max = *chunk
                    .iter()
                    .max_by(|a, b| a.value(metric).total_cmp(&b.value(metric)))
                    .unwrap();
                if min.elapsed_s <= max.elapsed_s {
                    points.extend([min, max]);
                } else {
                    points.extend([max, min]);
                }
            }
            points.push(*segment.last().unwrap());
            points.dedup_by(|a, b| a.elapsed_s == b.elapsed_s);
            points
        })
        .collect()
}
fn zoom_range(range: (f64, f64), anchor: f64, factor: f64) -> (f64, f64) {
    let span = range.1 - range.0;
    let next_span = (span * factor).clamp(0.000001, 1e9);
    let point = range.0 + span * anchor;
    (
        point - next_span * anchor,
        point + next_span * (1. - anchor),
    )
}

struct ChannelView<'a> {
    time: (f64, f64),
    history: bool,
    zero_baseline: bool,
    height: f32,
    viewport: &'a mut ChartViewport,
}
fn channel_view(
    ui: &mut egui::Ui,
    samples: &[&Sample],
    latest: Option<&Sample>,
    mut view: ChannelView<'_>,
    selected: &mut usize,
    l: &I18n,
) {
    let (outer, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), view.height + 28.),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();
    crate::theme::accessible_label(&response, l.tr("Measurement charts"));
    #[cfg(test)]
    ui.data_mut(|data| data.insert_temp(egui::Id::new("measurement-charts"), outer));
    let labels = if outer.width() < 650. { 140. } else { 170. };
    let row_height = view.height / 3.;
    let trace_left = outer.left() + labels + 60.;
    let trace_right = outer.right() - 16.;
    let pointer = response
        .hover_pos()
        .filter(|p| p.x >= trace_left && p.x <= trace_right && p.y < outer.top() + view.height);
    let drag_origin = if response.dragged() {
        ui.input(|input| input.pointer.press_origin())
    } else {
        None
    };
    let active_metric = drag_origin
        .or(response.interact_pointer_pos())
        .or(pointer)
        .map(|p| (((p.y - outer.top()) / row_height) as usize).min(2));
    if pointer.is_some() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    if response.double_clicked() {
        *view.viewport = ChartViewport::default();
    } else if let Some(metric) = active_metric {
        let range = view.viewport.values[metric]
            .unwrap_or_else(|| value_range(samples, metric, view.zero_baseline));
        let plot_height = (row_height - 32.).max(1.);
        if response.dragged_by(egui::PointerButton::Primary)
            && response
                .interact_pointer_pos()
                .is_some_and(|p| p.x >= trace_left)
        {
            let delta = response.drag_delta();
            let shift =
                -delta.x as f64 / (trace_right - trace_left) as f64 * (view.time.1 - view.time.0);
            view.time = (view.time.0 + shift, view.time.1 + shift);
            view.viewport.time = Some(view.time);
            let shift_y = delta.y as f64 / plot_height as f64 * (range.1 - range.0);
            view.viewport.values[metric] = Some((range.0 + shift_y, range.1 + shift_y));
        }
        if let Some(pointer) = pointer {
            let (scroll, pinch) = ui.input_mut(|input| {
                let scroll = input.smooth_scroll_delta;
                let pinch = input.zoom_delta();
                input.smooth_scroll_delta = egui::Vec2::ZERO;
                input.raw_scroll_delta = egui::Vec2::ZERO;
                (scroll, pinch)
            });
            let factor = ((-scroll.y as f64 * 0.005).exp() / pinch as f64).clamp(0.2, 5.);
            if (factor - 1.).abs() > 0.00001 {
                let anchor = ((pointer.x - trace_left) / (trace_right - trace_left)) as f64;
                view.time = zoom_range(view.time, anchor, factor);
                view.viewport.time = Some(view.time);
                let top = outer.top() + metric as f32 * row_height + 16.;
                let anchor_y = (1. - (pointer.y - top) / plot_height).clamp(0., 1.) as f64;
                view.viewport.values[metric] = Some(zoom_range(range, anchor_y, factor));
            }
            if scroll.x != 0. {
                let shift = scroll.x as f64 / (trace_right - trace_left) as f64
                    * (view.time.1 - view.time.0);
                view.time = (view.time.0 + shift, view.time.1 + shift);
                view.viewport.time = Some(view.time);
            }
        }
    }
    let hovered = pointer.and_then(|p| {
        let t = view.time.0
            + (p.x - trace_left) as f64 / (trace_right - trace_left) as f64
                * (view.time.1 - view.time.0);
        samples
            .iter()
            .min_by(|a, b| (a.elapsed_s - t).abs().total_cmp(&(b.elapsed_s - t).abs()))
            .copied()
    });
    for metric in 0..3 {
        let top = outer.top() + metric as f32 * row_height;
        let row = egui::Rect::from_min_size(
            egui::pos2(outer.left(), top),
            egui::vec2(outer.width(), row_height),
        );
        let label = egui::Rect::from_min_size(row.min, egui::vec2(labels, row_height));
        painter.rect_filled(row, 0, Color32::from_rgb(25, 27, 30));
        painter.rect_filled(
            label,
            0,
            if *selected == metric {
                Color32::from_rgb(46, 50, 55)
            } else {
                crate::PANEL
            },
        );
        painter.rect_filled(
            egui::Rect::from_min_size(label.min, egui::vec2(3., row_height)),
            0,
            COLORS[metric],
        );
        painter.line_segment(
            [row.left_bottom(), row.right_bottom()],
            Stroke::new(1.0_f32, BORDER),
        );
        painter.text(
            label.min + egui::vec2(14., 18_f32.min(row_height * 0.2)),
            egui::Align2::LEFT_CENTER,
            l.tr(NAMES[metric]),
            egui::FontId::proportional(13.),
            COLORS[metric],
        );
        let observed = hovered.or(latest);
        let reading = observed
            .map(|s| format!("{:.3}", s.value(metric)))
            .unwrap_or_else(|| "—".into());
        painter.text(
            label.min + egui::vec2(14., row_height * 0.5),
            egui::Align2::LEFT_CENTER,
            reading,
            egui::FontId::monospace(22.),
            crate::theme::TEXT,
        );
        painter.text(
            egui::pos2(label.right() - 12., top + row_height * 0.5),
            egui::Align2::RIGHT_CENTER,
            UNITS[metric],
            egui::FontId::monospace(12.),
            MUTED,
        );
        painter.text(
            label.min + egui::vec2(14., row_height * 0.8),
            egui::Align2::LEFT_CENTER,
            l.tr(if metric == 1 {
                "INA bus voltage"
            } else {
                "INA supply branch"
            }),
            egui::FontId::proportional(11.),
            MUTED,
        );
        let plot = egui::Rect::from_min_max(
            egui::pos2(trace_left, top + 16.),
            egui::pos2(trace_right, top + row_height - 16.),
        );
        let range = view.viewport.values[metric]
            .unwrap_or_else(|| value_range(samples, metric, view.zero_baseline));
        for step in 0..=2 {
            let fraction = step as f32 / 2.;
            let y = plot.bottom() - fraction * plot.height();
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                Stroke::new(1.0_f32, Color32::from_rgb(43, 46, 51)),
            );
            painter.text(
                egui::pos2(plot.left() - 9., y),
                egui::Align2::RIGHT_CENTER,
                format!(
                    "{:.*}",
                    if range.1 - range.0 < 0.1 { 3 } else { 2 },
                    range.0 + fraction as f64 * (range.1 - range.0)
                ),
                egui::FontId::monospace(10.),
                MUTED,
            );
        }
        for tick in 0..=4 {
            let x = plot.left() + tick as f32 / 4. * plot.width();
            painter.line_segment(
                [egui::pos2(x, top), egui::pos2(x, top + row_height)],
                Stroke::new(1.0_f32, Color32::from_rgb(43, 46, 51)),
            );
        }
        draw_trace(
            painter,
            plot,
            samples,
            metric,
            COLORS[metric],
            view.time,
            range,
        );
        if samples.is_empty() && metric == 1 {
            painter.text(
                plot.center(),
                egui::Align2::CENTER_CENTER,
                l.tr("Waiting for real device measurements"),
                egui::FontId::proportional(12.),
                MUTED,
            );
        }
        if let Some(p) = pointer {
            painter.line_segment(
                [egui::pos2(p.x, top), egui::pos2(p.x, top + row_height)],
                Stroke::new(1.0_f32, MUTED),
            );
            if let Some(s) = hovered {
                painter.with_clip_rect(plot).circle_filled(
                    position(plot, s, metric, view.time, range),
                    2.5,
                    COLORS[metric],
                );
            }
        }
        if response.clicked()
            && response
                .interact_pointer_pos()
                .is_some_and(|p| label.contains(p))
        {
            *selected = metric;
        }
    }
    for tick in 0..=4 {
        let fraction = tick as f64 / 4.;
        let x = trace_left + fraction as f32 * (trace_right - trace_left);
        let label = if view.history || view.viewport.time.is_some() {
            format!(
                "{:.1} s",
                view.time.0 + (view.time.1 - view.time.0) * fraction
            )
        } else if tick == 4 {
            l.tr("Now").into()
        } else {
            format!("−{:.0} s", (view.time.1 - view.time.0) * (1. - fraction))
        };
        painter.text(
            egui::pos2(x, outer.top() + view.height + 15.),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::monospace(11.),
            MUTED,
        );
    }
    if let Some(sample) = hovered {
        response.on_hover_ui(|ui| {
            ui.monospace(format!("{:.3} s", sample.elapsed_s));
            for metric in 0..3 {
                ui.label(format!(
                    "{} {:.3} {}",
                    l.tr(NAMES[metric]),
                    sample.value(metric),
                    UNITS[metric]
                ));
            }
            ui.separator();
            ui.label(l.tr("Drag to pan; scroll to zoom; double-click to fit."));
        });
    } else {
        response.on_hover_text(l.tr("Drag to pan; scroll to zoom; double-click to fit."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clicking_overview_pause_freezes_readings_until_resume() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut device = Device::base("test".into(), "Probe".into());
        device.online = true;
        device.live_ready = true;
        let sample = |time, power| Sample {
            timestamp_unix_ms: 1700000000000 + (time * 1000.) as u64,
            elapsed_s: time,
            bus_voltage_v: 3.3,
            work_3v3_v: 3.3,
            current_ma: 20.,
            power_mw: power,
            power_on: true,
            power_fault: false,
        };
        let dir =
            std::env::temp_dir().join(format!("picoharness-overview-pause-{}", std::process::id()));
        device.monitor.start("test", &dir).unwrap();
        device.monitor.push(sample(1., 120.123));
        let render = |device: &mut Device, events| {
            ctx.data_mut(|data| {
                data.remove::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            });
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1360., 940.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show(ui, device, &I18n::default(), 2., &mut String::new());
                    });
                },
            )
        };
        render(&mut device, vec![]);
        let position = ctx
            .data(|data| {
                data.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            })
            .unwrap()
            .into_iter()
            .find(|(label, _)| label == "Pause refresh")
            .unwrap()
            .1
            .center();
        for pressed in [true, false] {
            render(
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
        assert!(
            device.monitor.paused(),
            "the actual Overview button must pause the monitor"
        );
        device.monitor.push(sample(2., 987.654));
        assert_eq!(device.monitor.samples(2.).last().unwrap().power_mw, 120.123);
        assert_eq!(device.monitor.live.back().unwrap().power_mw, 987.654);
        assert_eq!(device.monitor.history().unwrap().samples.len(), 2);
        render(&mut device, vec![]);
        let resume = ctx
            .data(|data| {
                data.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            })
            .unwrap()
            .into_iter()
            .find(|(label, _)| label == "Resume refresh")
            .unwrap()
            .1
            .center();
        for pressed in [true, false] {
            render(
                &mut device,
                vec![
                    egui::Event::PointerMoved(resume),
                    egui::Event::PointerButton {
                        pos: resume,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(!device.monitor.paused());
        assert_eq!(device.monitor.samples(2.).last().unwrap().power_mw, 987.654);
        device.monitor.stop().unwrap();
        let path = device.monitor.history().unwrap().path.as_ref().unwrap();
        assert_eq!(telemetry::History::read(path).unwrap().samples.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn monitor_controls_align_in_english_and_chinese() {
        for width in [720., 960., 1360.] {
            for language in [0, 1] {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx);
                let locale = I18n {
                    selected: language,
                    ..Default::default()
                };
                let mut d = Device::base("test".into(), "Probe".into());
                let mut rects = Vec::new();
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 700.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            rects = controls(ui, &mut d, &locale, &mut String::new());
                        });
                    },
                );
                assert_eq!(rects.len(), 10);
                for (index, rect) in rects.iter().enumerate() {
                    assert!(
                        (rect.height() - crate::theme::CONTROL_HEIGHT).abs() < 0.1,
                        "control {index}: {rect:?}"
                    );
                    let expected_offset = if width < 900. && index >= 7 { 31. } else { 0. };
                    assert!(
                        (rect.center().y - rects[0].center().y - expected_offset).abs() < 0.1,
                        "control {index} has a different baseline: {rect:?}"
                    );
                    for other in &rects[index + 1..] {
                        assert!(
                            !rect.intersects(*other),
                            "controls overlap: {rect:?}, {other:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn long_history_keeps_spikes_and_disconnect_gaps_with_bounded_geometry() {
        let rows: Vec<_> = (0..10_000)
            .map(|i| Sample {
                timestamp_unix_ms: i * 500,
                elapsed_s: i as f64 * 0.5 + if i >= 5000 { 10. } else { 0. },
                bus_voltage_v: 5.,
                work_3v3_v: 3.3,
                current_ma: 20.,
                power_mw: if i == 1234 { 1000. } else { 100. },
                power_on: true,
                power_fault: false,
            })
            .collect();
        let refs: Vec<_> = rows.iter().collect();
        let segments = trace_segments(&refs, 0, (0., 5010.), 100.);
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().flatten().any(|s| s.power_mw == 1000.));
        assert!(segments.iter().map(Vec::len).sum::<usize>() < 220);
        assert!(segments
            .iter()
            .all(|s| s.windows(2).all(|p| p[1].elapsed_s > p[0].elapsed_s)));
        assert_eq!(segments[1][0].elapsed_s, 2510.);
    }
}
