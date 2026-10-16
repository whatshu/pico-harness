use eframe::egui::{self, Color32, RichText};

pub const CONTROL_HEIGHT: f32 = 26.;
pub const BACKGROUND: Color32 = Color32::from_rgb(31, 33, 36);
pub const PANEL: Color32 = Color32::from_rgb(39, 41, 45);
pub const BORDER: Color32 = Color32::from_rgb(66, 69, 75);
pub const TEXT: Color32 = Color32::from_rgb(216, 218, 222);
pub const MUTED: Color32 = Color32::from_rgb(148, 153, 161);
pub const ACCENT: Color32 = Color32::from_rgb(133, 176, 208);
// Brighter accent for a channel that is actively carrying traffic.
pub const HOT: Color32 = Color32::from_rgb(186, 226, 255);
pub const RED: Color32 = Color32::from_rgb(219, 114, 114);

pub fn install(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BACKGROUND;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(25, 27, 30);
    v.faint_bg_color = Color32::from_rgb(43, 46, 50);
    v.selection.bg_fill = Color32::from_rgb(53, 67, 78);
    v.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.bg_fill = Color32::from_rgb(49, 52, 57);
    v.widgets.hovered.bg_fill = Color32::from_rgb(61, 66, 73);
    v.widgets.active.bg_fill = Color32::from_rgb(57, 72, 85);
    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        widget.corner_radius = egui::CornerRadius::same(2);
        widget.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    }
    v.window_corner_radius = egui::CornerRadius::same(3);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6., 5.);
        s.spacing.button_padding = egui::vec2(8., 4.);
        s.spacing.interact_size = egui::vec2(40., CONTROL_HEIGHT);
        s.spacing.icon_width = 14.;
        s.spacing.icon_spacing = 5.;
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.));
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(17.));
    });
}

pub fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0_f32, BORDER))
        .corner_radius(2)
        .inner_margin(10)
}

/// A terminal viewport fills its allocation even when there is no output yet.
pub fn terminal(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    lines: &std::collections::VecDeque<String>,
    height: f32,
) -> egui::Rect {
    let width = ui.available_width();
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0_f32, BORDER))
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_min_width((width - 12.).max(0.));
            let height = (height - 12.).max(30.);
            egui::ScrollArea::both()
                .id_salt(id)
                .auto_shrink([false, false])
                .min_scrolled_height(height)
                .max_height(height)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for line in lines {
                        ui.add(egui::Label::new(RichText::new(line).monospace()).extend());
                    }
                });
        })
        .response
        .rect
}

pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(format!("● {text}")).color(color).size(11.));
}

pub fn row<R>(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        contents,
    )
}

pub fn button(ui: &mut egui::Ui, label: &str, width: f32, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(label).min_size(egui::vec2(width, CONTROL_HEIGHT)),
    );
    #[cfg(test)]
    capture(ui, label, response.rect);
    response
}

/// Shared vector icons stay sharp at every DPI and need no icon font.
#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Rising,
    Falling,
    NoTrigger,
    Decoder,
    Eye,
    ZoomIn,
    ZoomOut,
    Fit,
    Cursors,
    Export,
    Trash,
    Trace,
    Devices,
    Settings,
    Scan,
    Refresh,
    Power,
    Close,
    Hardware,
    Uart,
    Spi,
    ScrollToBottom,
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    icon_toggle(ui, icon, label, false, enabled)
}

pub fn icon_toggle(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .selected(selected)
            .min_size(egui::vec2(30., CONTROL_HEIGHT)),
    );
    decorate_icon(ui, &response, icon, label, selected);
    response.on_hover_text(label)
}

pub fn icon_menu<R>(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let mut response = egui::menu::menu_custom_button(
        ui,
        egui::Button::new("").min_size(egui::vec2(30., CONTROL_HEIGHT)),
        contents,
    );
    decorate_icon(ui, &response.response, icon, label, false);
    response.response = response.response.on_hover_text(label);
    response
}

fn decorate_icon(
    ui: &egui::Ui,
    response: &egui::Response,
    icon: Icon,
    label: &str,
    selected: bool,
) {
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            response.enabled(),
            selected,
            label,
        )
    });
    #[cfg(test)]
    capture(ui, label, response.rect);
    let color = if selected {
        ACCENT
    } else {
        ui.style().interact(response).fg_stroke.color
    };
    paint_icon(ui, response, icon, color);
}

/// Connection indicators reveal their full status on hover.
pub fn status_icon(ui: &mut egui::Ui, icon: Icon, label: &str, detail: &str, color: Color32) {
    let (_, response) = ui.allocate_exact_size(
        egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    accessible_label(&response, label);
    #[cfg(test)]
    capture(ui, label, response.rect);
    paint_icon(ui, &response, icon, color);
    response.on_hover_text(format!("{label}\n{detail}"));
}

fn paint_icon(ui: &egui::Ui, response: &egui::Response, icon: Icon, color: Color32) {
    let stroke = egui::Stroke::new(1.4_f32, color);
    let p = ui.painter();
    let center = response.rect.center();
    let pos = |x, y| center + egui::vec2(x, y);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| pos(x, y)).collect(),
            stroke,
        ));
    };
    let box_outline = |min: (f32, f32), max: (f32, f32)| {
        p.rect_stroke(
            egui::Rect::from_min_max(pos(min.0, min.1), pos(max.0, max.1)),
            1.,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![pos(-4., -6.), pos(6., 0.), pos(-4., 6.)],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for x in [-4., 2.] {
                p.rect_filled(
                    egui::Rect::from_min_size(pos(x, -6.), egui::vec2(3., 12.)),
                    0.,
                    color,
                );
            }
        }
        Icon::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(center, egui::vec2(10., 10.)),
                1.,
                color,
            );
        }
        Icon::Rising | Icon::Falling => {
            let sign = if matches!(icon, Icon::Rising) {
                1.
            } else {
                -1.
            };
            line(&[
                (-7., 5. * sign),
                (0., 5. * sign),
                (0., -5. * sign),
                (7., -5. * sign),
            ]);
            line(&[(-3., -2. * sign), (0., -5. * sign), (3., -2. * sign)]);
        }
        Icon::NoTrigger => {
            line(&[(-7., 0.), (7., 0.)]);
            p.circle_stroke(center, 6., stroke);
            line(&[(-4., 4.), (4., -4.)]);
        }
        Icon::Decoder => {
            line(&[(-3., -5.), (-7., 0.), (-3., 5.)]);
            line(&[(3., -5.), (7., 0.), (3., 5.)]);
            line(&[(1., -6.), (-1., 6.)]);
        }
        Icon::Eye => {
            line(&[
                (-8., 0.),
                (-4., -4.),
                (4., -4.),
                (8., 0.),
                (4., 4.),
                (-4., 4.),
                (-8., 0.),
            ]);
            p.circle_stroke(center, 2., stroke);
        }
        Icon::Scan | Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(pos(-2., -2.), 5., stroke);
            line(&[(2., 2.), (7., 7.)]);
            if !matches!(icon, Icon::Scan) {
                line(&[(-5., -2.), (1., -2.)]);
            }
            if matches!(icon, Icon::ZoomIn) {
                line(&[(-2., -5.), (-2., 1.)]);
            }
        }
        Icon::Fit => {
            for sign in [-1., 1.] {
                line(&[(sign * 6., -6.), (sign * 6., 6.)]);
                line(&[(sign * 2., -3.), (sign * 5., 0.), (sign * 2., 3.)]);
            }
            line(&[(-5., 0.), (5., 0.)]);
        }
        Icon::Cursors => {
            for x in [-4., 4.] {
                line(&[(x, -7.), (x, 7.)]);
                line(&[(x - 2., -7.), (x + 2., -7.)]);
            }
        }
        Icon::Export => {
            line(&[(-6., 2.), (-6., 6.), (6., 6.), (6., 2.)]);
            line(&[(0., -7.), (0., 2.)]);
            line(&[(-3., -1.), (0., 2.), (3., -1.)]);
        }
        Icon::Trash => {
            line(&[(-7., -4.), (7., -4.)]);
            line(&[(-3., -5.), (-3., -7.), (3., -7.), (3., -5.)]);
            line(&[(-5., -2.), (-4., 7.), (4., 7.), (5., -2.)]);
            for x in [-2., 2.] {
                line(&[(x, -1.), (x, 4.)]);
            }
        }
        Icon::Trace => {
            line(&[
                (-8., 4.),
                (-4., 4.),
                (-4., -4.),
                (0., -4.),
                (0., 4.),
                (4., 4.),
                (4., -4.),
                (8., -4.),
            ]);
        }
        Icon::Devices => {
            box_outline((-7., -6.), (7., 6.));
            line(&[(-2., -5.), (-2., 5.)]);
        }
        Icon::Settings => {
            p.circle_stroke(center, 5., stroke);
            p.circle_stroke(center, 2., stroke);
            for (x, y) in [
                (0., 7.),
                (7., 0.),
                (0., -7.),
                (-7., 0.),
                (5., 5.),
                (5., -5.),
                (-5., 5.),
                (-5., -5.),
            ] {
                line(&[(x * 0.7, y * 0.7), (x, y)]);
            }
        }
        Icon::Refresh => {
            // A true circular arc, sampled densely for high-DPI windows.
            let start = -std::f32::consts::FRAC_PI_2;
            let sweep = std::f32::consts::TAU * 0.85;
            let points: Vec<_> = (0..=64)
                .map(|i| {
                    let angle = start + sweep * i as f32 / 64.;
                    (6. * angle.cos(), 6. * angle.sin())
                })
                .collect();
            line(&points);
            let angle = start + sweep;
            let tip = egui::vec2(6. * angle.cos(), 6. * angle.sin());
            let tangent = egui::vec2(-angle.sin(), angle.cos());
            let normal = egui::vec2(angle.cos(), angle.sin());
            let base = tip - tangent * 3.5;
            let a = base + normal * 2.5;
            let b = base - normal * 2.5;
            line(&[(a.x, a.y), (tip.x, tip.y), (b.x, b.y)]);
        }
        Icon::Power => {
            let points: Vec<_> = (0..=20)
                .map(|i| {
                    let a = std::f32::consts::PI * (0.2 + i as f32 * 1.6 / 20.);
                    (a.sin() * 6., -a.cos() * 6.)
                })
                .collect();
            line(&points);
            line(&[(0., -8.), (0., 0.)]);
        }
        Icon::Close => {
            line(&[(-5., -5.), (5., 5.)]);
            line(&[(-5., 5.), (5., -5.)]);
        }
        Icon::Hardware => {
            box_outline((-5., -5.), (5., 5.));
            for offset in [-3., 0., 3.] {
                line(&[(offset, -8.), (offset, -5.)]);
                line(&[(offset, 5.), (offset, 8.)]);
                line(&[(-8., offset), (-5., offset)]);
                line(&[(5., offset), (8., offset)]);
            }
        }
        Icon::Uart => {
            line(&[(-7., -3.), (7., -3.)]);
            line(&[(3., -6.), (7., -3.), (3., 0.)]);
            line(&[(7., 3.), (-7., 3.)]);
            line(&[(-3., 0.), (-7., 3.), (-3., 6.)]);
        }
        Icon::Spi => {
            box_outline((-3., -6.), (3., 6.));
            for offset in [-4., 0., 4.] {
                line(&[(-8., offset), (-3., offset)]);
                line(&[(3., offset), (8., offset)]);
            }
        }
        Icon::ScrollToBottom => {
            line(&[(0., -7.), (0., 3.)]);
            line(&[(-4., -1.), (0., 3.), (4., -1.)]);
            line(&[(-7., 7.), (7., 7.)]);
        }
    }
}

/// Name a text field without adding another visible caption.
pub fn accessible_label(response: &egui::Response, label: &str) {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_label(label));
}

pub fn refresh_button(ui: &mut egui::Ui, paused: bool, enabled: bool) -> egui::Response {
    icon_button(
        ui,
        if paused { Icon::Play } else { Icon::Pause },
        if paused {
            "Resume refresh"
        } else {
            "Pause refresh"
        },
        enabled,
    )
}

pub fn text_field(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    text_field_sized(ui, text, 0.)
}

pub fn text_field_sized(ui: &mut egui::Ui, text: &mut String, width: f32) -> egui::Response {
    let margin = egui::Margin::symmetric(4, 2);
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .margin(margin)
            .min_size(egui::vec2(width, CONTROL_HEIGHT)),
    );
    #[cfg(test)]
    // egui 0.31 returns the inner text rect; alignment uses the visible frame.
    capture(ui, "Text field", response.rect + margin);
    response
}

pub fn field_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(156., CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(156.);
            ui.label(label);
        },
    );
}

pub fn toggle(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(label)
            .selected(selected)
            .min_size(egui::vec2(width, CONTROL_HEIGHT)),
    );
    #[cfg(test)]
    capture(ui, label, response.rect);
    response
}

#[cfg(test)]
fn capture(ui: &egui::Ui, label: &str, rect: egui::Rect) {
    ui.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("toolbar_geometry"))
            .push((label.into(), rect))
    });
}

pub fn tab(ui: &mut egui::Ui, label: &str, width: f32, selected: bool) -> egui::Response {
    let response = ui.add_sized(
        [width, 28.],
        egui::Button::new(RichText::new(label).color(if selected { TEXT } else { MUTED }))
            .frame(false),
    );
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            egui::Stroke::new(2.0_f32, ACCENT),
        );
    }
    response
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    #[test]
    fn terminal_fills_the_available_width_and_height_even_when_empty() {
        for size in [egui::vec2(680., 500.), egui::vec2(1360., 940.)] {
            let ctx = egui::Context::default();
            install(&ctx);
            for _ in 0..2 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let available = ui.available_rect_before_wrap();
                            let rect =
                                terminal(ui, "empty", &Default::default(), available.height());
                            assert!(
                                (rect.width() - available.width()).abs() <= 2.,
                                "{rect:?}, {available:?}"
                            );
                            assert!(
                                (rect.height() - available.height()).abs() <= 2.,
                                "{rect:?}, {available:?}"
                            );
                        });
                    },
                );
            }
        }
    }
}
