use crate::{
    file_pick,
    gdb::{
        self,
        mi::{quote, Value},
    },
    i18n::I18n,
    model::{DebugView, Device},
    ACCENT, MUTED, RED,
};
use eframe::egui::{self, Color32, RichText};

pub fn show(ui: &mut egui::Ui, d: &mut Device, l: &I18n) {
    ui.set_min_width(ui.available_width());
    let flashing = d.flash.job.is_some() || d.swd || d.swd_pending;
    let now = ui.input(|input| input.time);
    let mut start = false;
    let view = &mut d.debugger;
    let connection_id = ui.id().with("connection-session");
    let had_session = ui
        .data(|data| data.get_temp::<bool>(connection_id))
        .unwrap_or(false);
    let connected = view.session.is_some();
    ui.data_mut(|data| data.insert_temp(connection_id, connected));
    egui::CollapsingHeader::new(l.tr("Debug connection"))
        .default_open(view.session.is_none())
        .open((connected && !had_session).then_some(false))
        .show(ui, |ui| {
            ui.add_enabled_ui(view.session.is_none(), |ui| {
                ui.checkbox(
                    &mut view.config.server_only,
                    l.tr("GDB server only (VS Code / external debugger)"),
                );
                if view.config.server_only {
                    view.config.managed = true;
                }
                let field_width = (ui.available_width() - 300.).clamp(100., 560.);
                ui.spacing_mut().text_edit_width = field_width;
                egui::Grid::new("gdb-config")
                    .num_columns(3)
                    .spacing([12., 6.])
                    .show(ui, |ui| {
                        crate::theme::field_label(ui, l.tr("ELF with debug symbols"));
                        crate::theme::text_field_sized(ui, &mut view.config.elf, field_width);
                        if ui.button(l.tr("Browse")).clicked() {
                            if let Some(p) = file_pick(&["elf", "axf"]) {
                                view.config.elf = p;
                            }
                        }
                        ui.end_row();
                        crate::theme::field_label(ui, "GDB");
                        crate::theme::text_field_sized(ui, &mut view.config.gdb, field_width);
                        ui.label(l.tr("Executable path"));
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut view.config.managed, false, l.tr("Existing OpenOCD"));
                    ui.selectable_value(
                        &mut view.config.managed,
                        true,
                        l.tr("Start local OpenOCD"),
                    );
                });
                egui::Grid::new("openocd-config")
                    .num_columns(3)
                    .spacing([12., 6.])
                    .show(ui, |ui| {
                        if view.config.managed {
                            crate::theme::field_label(ui, l.tr("Probe serial"));
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.serial,
                                field_width,
                            );
                            egui::ComboBox::from_id_salt("target")
                                .selected_text(&view.config.target)
                                .show_ui(ui, |ui| {
                                    for t in ["rp2040", "rp2350"] {
                                        ui.selectable_value(&mut view.config.target, t.into(), t);
                                    }
                                });
                            ui.end_row();
                            crate::theme::field_label(ui, "OpenOCD");
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.openocd,
                                field_width,
                            );
                            ui.end_row();
                            crate::theme::field_label(ui, l.tr("Scripts directory"));
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.scripts,
                                field_width,
                            );
                            ui.end_row();
                        } else {
                            crate::theme::field_label(ui, l.tr("Host:port"));
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.endpoint,
                                field_width,
                            );
                            ui.end_row();
                        }
                    });
                ui.collapsing(l.tr("Source path mapping"), |ui| {
                    ui.spacing_mut().text_edit_width = (field_width - 20.).max(100.);
                    egui::Grid::new("source-path-config")
                        .num_columns(3)
                        .spacing([12., 6.])
                        .show(ui, |ui| {
                            crate::theme::field_label(ui, l.tr("Build path"));
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.source_from,
                                (field_width - 20.).max(100.),
                            );
                            ui.end_row();
                            crate::theme::field_label(ui, l.tr("Local source directory"));
                            crate::theme::text_field_sized(
                                ui,
                                &mut view.config.source_to,
                                (field_width - 20.).max(100.),
                            );
                            if ui.button(l.tr("Browse")).clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                    view.config.source_to = p.display().to_string();
                                }
                            }
                            ui.end_row();
                        });
                });
            });
            ui.horizontal(|ui| {
                if view.session.is_none()
                    && ui
                        .add_enabled(
                            (view.config.server_only || !view.config.elf.is_empty()) && !flashing,
                            egui::Button::new(RichText::new(l.tr("Connect")).color(ACCENT)),
                        )
                        .clicked()
                {
                    start = true;
                }
                ui.label(
                    RichText::new(
                        l.tr("Attach only · ELF must match the target · no automatic download"),
                    )
                    .color(MUTED)
                    .small(),
                );
            });
        });
    if start {
        d.stop_live(now);
        d.debugger.start();
    }
    let view = &mut d.debugger;
    if view.config.server_only && view.session.is_some() {
        disconnect_button(ui, view, l);
        ui.label(l.tr("External GDB endpoint"));
        if view.state == "Server running" {
            ui.monospace(&view.config.endpoint);
        } else {
            ui.label(l.tr(&view.state));
        }
        if ui
            .add_enabled(
                view.state == "Server running",
                egui::Button::new(l.tr("Copy connection command")),
            )
            .clicked()
        {
            ui.ctx()
                .copy_text(format!("target extended-remote {}", view.config.endpoint));
        }
        ui.separator();
        crate::theme::terminal(ui, "server-log", &view.openocd, ui.available_height());
        return;
    }
    if !view.error.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(RED, &view.error);
            if ui.small_button("×").clicked() {
                view.error.clear();
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        if view.session.is_some() {
            disconnect_button(ui, view, l);
            ui.separator();
        }
        let stopped = view.session.is_some() && view.state == "stopped";
        let running = view.session.is_some() && view.state == "running";
        for (label, cmd, enabled) in [
            ("Pause", "-exec-interrupt --all", running),
            ("Continue", "-exec-continue", stopped),
            ("Step into", "-exec-step", stopped),
            ("Step over", "-exec-next", stopped),
            ("Step out", "-exec-finish", stopped),
            ("Instruction", "-exec-step-instruction", stopped),
        ] {
            if ui
                .add_enabled(enabled, egui::Button::new(l.tr(label)))
                .clicked()
            {
                view.execute(cmd);
            }
        }
        if ui
            .add_enabled(stopped, egui::Button::new(l.tr("Reset & halt")))
            .clicked()
        {
            view.console_command("monitor reset halt");
        }
        if ui
            .add_enabled(stopped, egui::Button::new(l.tr("Refresh")))
            .clicked()
        {
            view.refresh();
        }
        ui.label(RichText::new(l.tr(&view.state)).color(if running { ACCENT } else { MUTED }));
        if view.session.is_none() && !view.data.is_empty() {
            ui.colored_label(RED, l.tr("Stale snapshot"));
        }
    });
    ui.separator();
    let main_height = (ui.available_height() * 0.65 - 20.).max(100.);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), main_height),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            let width = (ui.available_width() - 305.).max(300.);
            ui.allocate_ui_with_layout(
                egui::vec2(width, main_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    source(ui, view, l, main_height);
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), main_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    inspector(ui, view, l, main_height);
                },
            );
        },
    );
    ui.separator();
    ui.horizontal(|ui| {
        for (i, label) in ["GDB Console", "Memory", "Registers", "OpenOCD Log"]
            .iter()
            .enumerate()
        {
            ui.selectable_value(&mut view.bottom, i, l.tr(label));
        }
    });
    match view.bottom {
        0 => console(ui, view, l),
        1 => memory(ui, view, l),
        2 => registers(ui, view, l),
        _ => {
            crate::theme::terminal(ui, "openocd-log", &view.openocd, ui.available_height());
        }
    }
}

fn disconnect_button(ui: &mut egui::Ui, view: &mut DebugView, l: &I18n) {
    if ui
        .add_enabled(
            view.state != "Disconnecting",
            egui::Button::new(l.tr("Disconnect")),
        )
        .clicked()
    {
        view.state = "Disconnecting".into();
        if let Some(session) = &view.session {
            session.send(gdb::Request::Disconnect);
        }
    }
}
fn source(ui: &mut egui::Ui, v: &mut DebugView, l: &I18n, height: f32) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("source-files")
            .width(230.)
            .selected_text(if v.source_path.is_empty() {
                l.tr("Source file")
            } else {
                std::path::Path::new(&v.source_path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&v.source_path)
            })
            .show_ui(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut v.source_search)
                        .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                        .hint_text(l.tr("Search")),
                );
                let files = v.field("files", "files").to_vec();
                for file in files {
                    let path = if file.field("fullname").is_empty() {
                        file.field("file")
                    } else {
                        file.field("fullname")
                    };
                    if path
                        .to_lowercase()
                        .contains(&v.source_search.to_lowercase())
                        && ui.selectable_label(v.source_path == path, path).clicked()
                    {
                        v.follow = false;
                        v.read_source(path);
                    }
                }
            });
        if ui.checkbox(&mut v.follow, l.tr("Follow PC")).changed() && v.follow {
            if let Some(stack) = v.data.get("stack").cloned() {
                v.apply_stack(&stack);
            }
            let id = ui.id().with(("source-follow-generation", &v.source_path));
            ui.data_mut(|data| data.remove::<(u64, usize)>(id));
        }
        if ui.small_button(l.tr("Assembly")).clicked() {
            v.query(
                "assembly",
                "-data-disassemble -s \"$pc\" -e \"$pc+128\" -- 0".into(),
            );
            v.source.clear();
        }
    });
    let text = v.source.clone();
    let breakpoints = breakpoints(v);
    let connected = v.session.is_some() && matches!(v.state.as_str(), "stopped" | "running");
    let width = ui.available_width();
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(2)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_min_width((width - 12.).max(0.));
            let viewport_height = (height - 48.).max(40.);
            let row_height = crate::theme::CONTROL_HEIGHT;
            let mut scroll = egui::ScrollArea::both()
                .id_salt("source-scroll")
                .auto_shrink([false, false])
                .max_height(viewport_height)
                .min_scrolled_height(viewport_height);
            let follow_id = ui.id().with(("source-follow-generation", &v.source_path));
            let followed = ui.data(|data| data.get_temp::<(u64, usize)>(follow_id));
            if v.follow && v.source_line > 0 && followed != Some((v.generation, v.source_line)) {
                let line_offset =
                    (v.source_line - 1) as f32 * (row_height + ui.spacing().item_spacing.y);
                let centered_offset = (line_offset - viewport_height * 0.5).max(0.);
                scroll = scroll.vertical_scroll_offset(centered_offset);
                ui.data_mut(|data| data.insert_temp(follow_id, (v.generation, v.source_line)));
            }
            if text.is_empty() {
                scroll.show(ui, |ui| {
                    let message = l.tr(
                        "Source unavailable. Configure source path mapping, or inspect assembly.",
                    );
                    ui.label(RichText::new(message).color(MUTED));
                    if let Some(assembly) = v
                        .data
                        .get("assembly")
                        .and_then(|value| value.get("asm_insns"))
                    {
                        for instruction in assembly.items() {
                            ui.monospace(format!(
                                "{}  {}",
                                instruction.field("address"),
                                instruction.field("inst")
                            ));
                        }
                    }
                });
                return;
            }
            let lines: Vec<_> = text.lines().collect();
            scroll.show_rows(ui, row_height, lines.len(), |ui, visible| {
                for index in visible {
                    let line_number = index + 1;
                    let current = line_number == v.source_line;
                    let existing = breakpoints.iter().find(|breakpoint| {
                        let path = if breakpoint.field("fullname").is_empty() {
                            breakpoint.field("file")
                        } else {
                            breakpoint.field("fullname")
                        };
                        let path = std::path::Path::new(path);
                        let source_path = std::path::Path::new(&v.source_path);
                        let same_file = path == source_path
                            || (path.components().count() == 1
                                && path.file_name() == source_path.file_name());
                        same_file
                            && breakpoint.field("line").parse::<usize>().ok() == Some(line_number)
                    });
                    ui.horizontal(|ui| {
                        let (marker, color) = if existing.is_some() {
                            ("●", RED)
                        } else if current {
                            ("▶", ACCENT)
                        } else {
                            (" ", MUTED)
                        };
                        let gutter = egui::Button::new(
                            RichText::new(format!("{marker} {line_number:4}"))
                                .monospace()
                                .color(color),
                        )
                        .frame(false);
                        if ui.add_enabled(connected, gutter).clicked() {
                            if let Some(breakpoint) = existing {
                                v.breakpoint(format!(
                                    "-break-delete {}",
                                    breakpoint.field("number")
                                ));
                            } else {
                                let location = format!("{}:{line_number}", v.source_path);
                                v.breakpoint(format!("-break-insert {}", quote(&location)));
                            }
                        }
                        if current {
                            let text = RichText::new(lines[index])
                                .monospace()
                                .background_color(ui.visuals().selection.bg_fill)
                                .color(ACCENT);
                            ui.add(egui::Label::new(text).extend());
                        } else {
                            ui.add(egui::Label::new(highlight(lines[index])).extend());
                        }
                    });
                }
            });
        });
}

fn highlight(line: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut string = false;
    let mut comment = false;
    for word in line.split_inclusive(|c: char| !c.is_alphanumeric() && c != '_') {
        if word.contains("//") {
            comment = true;
        }
        let color = if comment || line.trim_start().starts_with("//") {
            MUTED
        } else if string || word.contains('"') {
            Color32::from_rgb(218, 181, 117)
        } else if [
            "int", "void", "uint32_t", "return", "while", "if", "else", "for", "const", "static",
            "struct", "bool", "fn", "let", "mut", "pub",
        ]
        .contains(&word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        {
            Color32::from_rgb(186, 151, 237)
        } else {
            Color32::from_rgb(209, 220, 236)
        };
        if word.chars().filter(|c| *c == '"').count() % 2 == 1 {
            string = !string;
        }
        job.append(
            word,
            0.,
            egui::TextFormat {
                font_id: egui::FontId::monospace(12.),
                color,
                ..Default::default()
            },
        );
    }
    job
}
fn breakpoints(v: &DebugView) -> Vec<Value> {
    v.data
        .get("breakpoints")
        .and_then(|v| v.get("BreakpointTable"))
        .and_then(|v| v.get("body"))
        .map(|v| {
            v.items()
                .iter()
                .map(|v| v.get("bkpt").unwrap_or(v).clone())
                .collect()
        })
        .unwrap_or_default()
}
fn inspector(ui: &mut egui::Ui, v: &mut DebugView, l: &I18n, height: f32) {
    ui.horizontal_wrapped(|ui| {
        for (i, label) in ["Stack", "Locals", "Watch", "Breakpoints"]
            .iter()
            .enumerate()
        {
            ui.selectable_value(&mut v.inspector, i, l.tr(label));
        }
    });
    ui.separator();
    let stopped = v.state == "stopped" && v.session.is_some();
    egui::ScrollArea::vertical()
        .id_salt("inspector-scroll")
        .max_height(height - 50.)
        .show(ui, |ui| match v.inspector {
            0 => {
                ui.label(RichText::new(l.tr("Threads")).color(MUTED));
                for t in v.field("threads", "threads").to_vec() {
                    let id = t.field("id");
                    if ui
                        .add_enabled(
                            stopped,
                            egui::Button::new(format!(
                                "#{id} {} {}",
                                t.field("name"),
                                t.field("state")
                            )),
                        )
                        .clicked()
                    {
                        v.execute(format!("-thread-select {id}"));
                    }
                }
                ui.separator();
                ui.label(RichText::new(l.tr("Call stack")).color(MUTED));
                for f in v.field("stack", "stack").to_vec() {
                    let f = f.get("frame").unwrap_or(&f);
                    let level = f.field("level");
                    if ui
                        .add_enabled(
                            stopped,
                            egui::Button::new(format!(
                                "#{level} {} :{}",
                                f.field("func"),
                                f.field("line")
                            )),
                        )
                        .clicked()
                    {
                        v.follow = false;
                        v.execute(format!("-stack-select-frame {level}"));
                        v.focus_frame(f);
                    }
                }
            }
            1 => {
                for var in v.field("locals", "variables").to_vec() {
                    let name = var.field("name");
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ACCENT, name);
                        ui.label(var.field("type"));
                        ui.monospace(var.field("value"));
                    });
                    if !name.is_empty() {
                        let key = format!("var:{name}");
                        if ui
                            .small_button(format!("{} {name}", l.tr("Expand")))
                            .clicked()
                            && stopped
                        {
                            v.query(&key, format!("-var-create - * {}", quote(name)));
                        }
                        if let Some(object) = v.data.get(&key).cloned() {
                            variable_tree(ui, v, &object, 0);
                        }
                    }
                    ui.separator();
                }
                ui.label(l.tr("Set expression"));
                crate::theme::text_field(ui, &mut v.expression);
                crate::theme::text_field(ui, &mut v.assignment);
                if ui
                    .add_enabled(stopped, egui::Button::new(l.tr("Write")))
                    .clicked()
                {
                    v.execute(format!(
                        "-data-evaluate-expression {}",
                        quote(&format!("{} = {}", v.expression, v.assignment))
                    ));
                }
            }
            2 => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut v.watch_input)
                            .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                            .desired_width(175.)
                            .hint_text(l.tr("Expression")),
                    );
                    if ui.button("+").clicked() && !v.watch_input.is_empty() {
                        v.watches.push(std::mem::take(&mut v.watch_input));
                        let i = v.watches.len() - 1;
                        v.query(
                            &format!("watch:{i}"),
                            format!("-data-evaluate-expression {}", quote(&v.watches[i])),
                        );
                    }
                });
                for (i, w) in v.watches.iter().enumerate() {
                    ui.label(RichText::new(w).color(ACCENT));
                    ui.monospace(
                        v.data
                            .get(&format!("watch:{i}"))
                            .map(|v| v.field("value"))
                            .unwrap_or("—"),
                    );
                }
                if ui.button(l.tr("Refresh")).clicked() {
                    for (i, w) in v.watches.iter().enumerate() {
                        v.query(
                            &format!("watch:{i}"),
                            format!("-data-evaluate-expression {}", quote(w)),
                        );
                    }
                }
            }
            _ => {
                for b in breakpoints(v) {
                    let id = b.field("number");
                    ui.horizontal_wrapped(|ui| {
                        let mut enabled = b.field("enabled") == "y";
                        if ui
                            .add_enabled(stopped, egui::Checkbox::new(&mut enabled, id))
                            .changed()
                        {
                            v.execute(format!(
                                "-break-{} {id}",
                                if enabled { "enable" } else { "disable" }
                            ));
                        }
                        ui.label(format!("{}:{}", b.field("func"), b.field("line")));
                        if ui.add_enabled(stopped, egui::Button::new("×")).clicked() {
                            v.breakpoint(format!("-break-delete {id}"));
                        }
                    });
                    if !b.field("cond").is_empty() {
                        ui.monospace(b.field("cond"));
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut v.condition)
                                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                                .hint_text(l.tr("Condition"))
                                .desired_width(160.),
                        );
                        if ui
                            .add_enabled(stopped, egui::Button::new(l.tr("Apply")))
                            .clicked()
                        {
                            v.breakpoint(format!("-break-condition {id} {}", quote(&v.condition)));
                        }
                    });
                    ui.separator();
                }
            }
        });
}
fn variable_tree(ui: &mut egui::Ui, v: &mut DebugView, object: &Value, depth: usize) {
    if depth > 6 {
        return;
    }
    let name = object.field("name");
    ui.monospace(object.field("value"));
    if object.field("numchild").parse::<usize>().unwrap_or(0) > 0 {
        let key = format!("children:{name}");
        egui::CollapsingHeader::new(if object.field("exp").is_empty() {
            name
        } else {
            object.field("exp")
        })
        .id_salt(name)
        .show(ui, |ui| {
            if let Some(children) = v.data.get(&key).and_then(|v| v.get("children")).cloned() {
                for child in children.items() {
                    let child = child.get("child").unwrap_or(child);
                    ui.horizontal(|ui| {
                        ui.colored_label(ACCENT, child.field("exp"));
                        ui.monospace(child.field("value"));
                    });
                    variable_tree(ui, v, child, depth + 1);
                }
            } else {
                v.query(
                    &key,
                    format!("-var-list-children --all-values {} 0 100", quote(name)),
                );
                ui.label("…");
            }
        });
    }
}
fn console(ui: &mut egui::Ui, v: &mut DebugView, l: &I18n) {
    crate::theme::terminal(ui, "gdb-console", &v.logs, ui.available_height() - 36.);
    ui.horizontal(|ui| {
        ui.monospace("(gdb)");
        let response = ui.add(
            egui::TextEdit::singleline(&mut v.console)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .desired_width(ui.available_width() - 85.)
                .font(egui::TextStyle::Monospace),
        );
        if (ui
            .add_enabled(v.session.is_some(), egui::Button::new(l.tr("Execute")))
            .clicked()
            || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            && !v.console.trim().is_empty()
        {
            v.console_command(&v.console);
            v.console.clear();
            response.request_focus();
        }
    });
}
fn memory(ui: &mut egui::Ui, v: &mut DebugView, l: &I18n) {
    let stopped = v.state == "stopped" && v.session.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.label(l.tr("Address"));
        let address = ui.add(
            egui::TextEdit::singleline(&mut v.memory_address)
                .id_salt("gdb-memory-address")
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .desired_width(150.),
        );
        #[cfg(test)]
        ui.data_mut(|data| data.insert_temp(egui::Id::new("gdb-memory-address-input"), address.id));
        ui.add(egui::DragValue::new(&mut v.memory_count).range(1..=4096));
        let read = ui
            .add_enabled(stopped, egui::Button::new(l.tr("Read")))
            .clicked();
        if stopped
            && (read
                || (address.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))))
        {
            v.read_memory();
        }
        ui.add(
            egui::TextEdit::singleline(&mut v.memory_write)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .hint_text("HEX")
                .desired_width(150.),
        );
        if ui
            .add_enabled(stopped, egui::Button::new(l.tr("Write")))
            .clicked()
        {
            match crate::pages::parse_hex(&v.memory_write) {
                Ok(bytes) if !bytes.is_empty() => {
                    v.execute(format!(
                        "-data-write-memory-bytes {} {}",
                        quote(v.memory_address.trim()),
                        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
                    ));
                    v.read_memory();
                }
                _ => v.error = l.tr("Invalid memory range or HEX data").into(),
            }
        }
    });
    let count = v.memory_read_count();
    if count < v.memory_count {
        ui.label(
            RichText::new(format!(
                "{}: {count}/{} {}",
                l.tr("Read limited to RAM boundary"),
                v.memory_count,
                l.tr("bytes")
            ))
            .small()
            .color(MUTED),
        );
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for block in v.field("memory", "memory") {
            let contents = block.field("contents");
            let base =
                u64::from_str_radix(block.field("begin").trim_start_matches("0x"), 16).unwrap_or(0);
            if let Ok(bytes) = crate::pages::parse_hex(contents) {
                for (i, row) in bytes.chunks(16).enumerate() {
                    ui.monospace(format!(
                        "{:08X}  {}  {}",
                        base + i as u64 * 16,
                        row.iter().map(|b| format!("{b:02X} ")).collect::<String>(),
                        row.iter()
                            .map(|b| if b.is_ascii_graphic() {
                                *b as char
                            } else {
                                '.'
                            })
                            .collect::<String>()
                    ));
                }
            }
        }
    });
}
fn registers(ui: &mut egui::Ui, v: &mut DebugView, l: &I18n) {
    let stopped = v.state == "stopped" && v.session.is_some();
    ui.horizontal(|ui| {
        ui.label(l.tr("Register number"));
        ui.add(
            egui::TextEdit::singleline(&mut v.register_index)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .desired_width(65.),
        );
        ui.add(
            egui::TextEdit::singleline(&mut v.register_write)
                .min_size(egui::vec2(0., crate::theme::CONTROL_HEIGHT))
                .hint_text("0x…")
                .desired_width(140.),
        );
        if ui
            .add_enabled(stopped, egui::Button::new(l.tr("Write")))
            .clicked()
        {
            match (
                v.register_index.parse::<u32>(),
                crate::pages::number(&v.register_write),
            ) {
                (Ok(i), Ok(value)) => {
                    v.execute(format!("-data-write-register-values x {i} 0x{value:x}"))
                }
                _ => v.error = l.tr("Invalid register value").into(),
            }
        }
    });
    let names = v.field("register_names", "register-names");
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("live-regs")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                for (i, r) in v.field("registers", "register-values").iter().enumerate() {
                    let number = r.field("number");
                    let name = number
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| names.get(i))
                        .map(Value::text)
                        .unwrap_or(number);
                    ui.label(
                        RichText::new(format!("{number}  {name}"))
                            .monospace()
                            .color(if v.state == "running" {
                                MUTED
                            } else {
                                ui.visuals().text_color()
                            }),
                    );
                    ui.label(RichText::new(r.field("value")).monospace().color(
                        if v.state == "running" {
                            MUTED
                        } else if v.changed_registers.iter().any(|s| s == number) {
                            Color32::YELLOW
                        } else {
                            ACCENT
                        },
                    ));
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ui-perf")]
    #[test]
    fn memory_write_and_tabs_remain_clickable_below_long_assembly() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut device = Device::base("test".into(), "Probe".into());
        let (session, _events, commands) = gdb::Session::test_source();
        device.debugger.session = Some(session);
        device.debugger.state = "stopped".into();
        device.debugger.memory_address = "0x20041FD0".into();
        device.debugger.memory_write = "11223344".into();
        let instructions = (0..64)
            .map(|i| format!("{{address=\"0x{:08x}\",inst=\"mov r0, r1\"}}", i * 2))
            .collect::<Vec<_>>()
            .join(",");
        let assembly = gdb::mi::parse(&format!("^done,asm_insns=[{instructions}]"))
            .unwrap()
            .unwrap();
        device
            .debugger
            .data
            .insert("assembly".into(), assembly.data);
        let render = |device: &mut Device, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1120., 820.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| show(ui, device, &I18n::default()));
                },
            )
        };
        for (label, expected_tab) in [("Memory", 1), ("Write", 1), ("GDB Console", 0)] {
            render(&mut device, vec![]);
            let output = render(&mut device, vec![]);
            let position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("Bottom control must be rendered");
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
            assert_eq!(
                device.debugger.bottom, expected_tab,
                "{label} must respond to clicks"
            );
            if label == "Write" {
                assert!(
                    commands.try_iter().any(|request| matches!(request,
                        gdb::Request::Execute(command)
                            if command == "-data-write-memory-bytes \"0x20041FD0\" 11223344"
                    )),
                    "Write must submit the selected address and bytes"
                );
            }
        }
    }

    #[cfg(feature = "ui-perf")]
    #[test]
    fn toolbar_refresh_reads_the_current_memory_address() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut device = Device::base("test".into(), "Probe".into());
        let (session, _events, commands) = gdb::Session::test_source();
        device.debugger.session = Some(session);
        device.debugger.state = "stopped".into();
        device.debugger.generation = 7;
        device.debugger.bottom = 1;
        let render = |device: &mut Device, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1120., 820.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| show(ui, device, &I18n::default()));
                },
            )
        };
        render(&mut device, vec![]);
        render(&mut device, vec![]);
        let input = ctx
            .data(|data| data.get_temp::<egui::Id>(egui::Id::new("gdb-memory-address-input")))
            .unwrap();
        ctx.memory_mut(|memory| memory.request_focus(input));
        let output = render(
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
                egui::Event::Text("0x20041FD0".into()),
            ],
        );
        assert_eq!(device.debugger.memory_address, "0x20041FD0");
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Refresh" => {
                    Some(text.pos + egui::vec2(4., 4.))
                }
                _ => None,
            })
            .expect("Toolbar Refresh must be visible");
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
            commands.try_iter().any(|request| matches!(request,
                gdb::Request::Query { key, command, generation: 7 }
                    if key == "memory" && command == "-data-read-memory-bytes \"0x20041FD0\" 48"
            )),
            "Refresh must read the edited address rather than retain the old memory block"
        );
    }

    #[test]
    fn source_split_keeps_the_bottom_console_visible_at_different_heights() {
        for height in [700., 940., 1200.] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut device = Device::base("test".into(), "Probe".into());
            for _ in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000., height),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default()
                            .show(ctx, |ui| show(ui, &mut device, &I18n::default()));
                    },
                );
                let execute = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == "Execute" => {
                            Some(text.pos)
                        }
                        _ => None,
                    })
                    .expect("GDB console input must be rendered");
                assert!(
                    execute.y < height - 8.,
                    "Console outside viewport: {execute:?}, height={height}"
                );
                assert!(
                    execute.y > height - 80.,
                    "Console must occupy the remaining height: {execute:?}, height={height}"
                );
            }
        }
    }

    #[test]
    fn connection_fields_align_and_fit_narrow_workspaces() {
        for width in [680., 944.] {
            for language in [0, 1] {
                for managed in [false, true] {
                    let ctx = egui::Context::default();
                    crate::theme::install(&ctx);
                    let locale = I18n {
                        selected: language,
                        ..Default::default()
                    };
                    let mut device = Device::base("test".into(), "Probe".into());
                    device.debugger.config.managed = managed;
                    let id = egui::Id::new("toolbar_geometry");
                    for _ in 0..2 {
                        ctx.data_mut(|data| {
                            data.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(id)
                                .clear()
                        });
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 700.),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default()
                                    .show(ctx, |ui| show(ui, &mut device, &locale));
                            },
                        );
                    }
                    let fields = ctx
                        .data(|data| data.get_temp::<Vec<(String, egui::Rect)>>(id))
                        .unwrap();
                    assert_eq!(fields.len(), if managed { 5 } else { 3 });
                    for (_, rect) in &fields {
                        assert!(rect.width() >= 100., "Path input collapsed: {rect:?}");
                        assert!(
                            rect.right() <= width - 8.,
                            "Path input outside workspace: {rect:?}"
                        );
                        assert!((rect.height() - crate::theme::CONTROL_HEIGHT).abs() < 0.1, "Field height: {rect:?}, width={width}, language={language}, managed={managed}");
                        assert!((rect.left() - fields[0].1.left()).abs() < 0.1);
                        assert!((rect.width() - fields[0].1.width()).abs() < 0.1);
                    }
                }
            }
        }
    }
}
