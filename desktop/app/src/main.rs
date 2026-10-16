#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use ph_app::gdb;
mod command_history;
mod gdb_ui;
#[cfg(test)]
mod gui_tests;
mod i18n;
mod live;
mod logic;
mod logic_ui;
mod model;
mod monitor_ui;
mod pages;
#[cfg(feature = "ui-perf")]
mod performance;
mod theme;
mod work_link;
use eframe::egui::{self, Color32, RichText};
use i18n::I18n;
use model::{Device, Page};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
pub use theme::{badge, channel_color, panel, ACCENT, BORDER, HOT, MUTED, PANEL, RED};
pub fn export(name: &str, bytes: impl AsRef<[u8]>) -> Result<(), String> {
    if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn file_pick(ext: &[&str]) -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Files", ext)
        .pick_file()
        .map(|p| p.display().to_string())
}
struct App {
    devices: Vec<Device>,
    selected: usize,
    locale: I18n,
    start: Instant,
    settings: bool,
    sidebar_open: bool,
    initial_page: Page,
    initial_swd_tab: usize,
    next_scan: f64,
    history_viewer: Device,
    show_history: bool,
    notice: String,
    scan: Option<mpsc::Receiver<Result<Vec<ph_app_link::DeviceInfo>, String>>>,
    screenshot: Option<String>,
    screenshot_requested: bool,
    quit_at: Option<f64>,
    displayed_page: Option<(String, Page)>,
    #[cfg(feature = "ui-perf")]
    performance: Option<performance::Runtime>,
}
impl App {
    fn page_shortcuts(&mut self, ctx: &egui::Context) {
        if self.settings || self.show_history {
            return;
        }
        let Some(device) = self.devices.get_mut(self.selected) else {
            return;
        };
        let selected = ctx.input_mut(|input| {
            for (index, key) in [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
            ]
            .into_iter()
            .enumerate()
            {
                if input.consume_key(egui::Modifiers::CTRL, key) {
                    return Some(Page::ALL[index]);
                }
            }
            let direction = if input.consume_key(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Tab,
            ) {
                -1
            } else if input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab) {
                1
            } else {
                return None;
            };
            let index = Page::ALL
                .iter()
                .position(|page| *page == device.page)
                .unwrap_or(0);
            Some(
                Page::ALL
                    [(index as isize + direction).rem_euclid(Page::ALL.len() as isize) as usize],
            )
        });
        if let Some(page) = selected {
            device.page = page;
            ctx.memory_mut(|memory| memory.close_popup());
        }
    }

    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".into(),
            egui::FontData::from_static(include_bytes!("../assets/StationCJK.otf")).into(),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .push("cjk".into());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("cjk".into());
        ctx.set_fonts(fonts);
        let mut app = Self {
            devices: Vec::new(),
            selected: 0,
            locale: Default::default(),
            start: Instant::now(),
            settings: false,
            sidebar_open: false,
            initial_page: Page::Overview,
            initial_swd_tab: 0,
            next_scan: 0.,
            history_viewer: Device::base("history".into(), "Saved history".into()),
            show_history: false,
            notice: String::new(),
            scan: None,
            screenshot: None,
            screenshot_requested: false,
            quit_at: None,
            displayed_page: None,
            #[cfg(feature = "ui-perf")]
            performance: None,
        };
        let args: Vec<_> = std::env::args().collect();
        app.sidebar_open = args.iter().any(|arg| arg == "--show-devices");
        for pair in args.windows(2) {
            match pair[0].as_str() {
                "--page" => {
                    if let Some(p) = Page::ALL
                        .into_iter()
                        .find(|p| p.name().eq_ignore_ascii_case(&pair[1]))
                    {
                        app.initial_page = p;
                    }
                }
                "--swd-tab" => {
                    app.initial_swd_tab = match pair[1].as_str() {
                        "memory" => 1,
                        "registers" => 2,
                        _ => 0,
                    }
                }
                "--screenshot" => app.screenshot = Some(pair[1].clone()),
                "--quit-after" => app.quit_at = pair[1].parse().ok(),
                "--language" => {
                    if pair[1] == "zh-CN" {
                        app.locale.selected = 1;
                    }
                }
                "--history" => match app
                    .history_viewer
                    .monitor
                    .load(std::path::Path::new(&pair[1]))
                {
                    Ok(()) => app.show_history = true,
                    Err(error) => app.notice = error,
                },
                _ => {}
            }
        }
        #[cfg(feature = "ui-perf")]
        if let Some(config) = performance::Config::parse(&args).expect("validated performance args")
        {
            let mut runtime = performance::Runtime::new(config, cc);
            runtime
                .install(&mut app)
                .expect("performance fixture setup");
            app.performance = Some(runtime);
        }
        if !cfg!(test) && app.discovery_enabled() {
            app.scan();
        }
        app
    }
    fn scan(&mut self) {
        if self.scan.is_some() || !self.discovery_enabled() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.scan = Some(rx);
        self.next_scan = self.start.elapsed().as_secs_f64() + 2.;
        std::thread::spawn(move || {
            let _ = tx.send(ph_app_link::enumerate_devices().map_err(|e| e.to_string()));
        });
    }
    fn discovery_enabled(&self) -> bool {
        #[cfg(feature = "ui-perf")]
        if let Some(runtime) = &self.performance {
            return runtime.config.scenario == "connected-live";
        }
        true
    }
    fn apply_discovery(&mut self, found: Vec<ph_app_link::DeviceInfo>) {
        #[cfg(feature = "ui-perf")]
        let found = found
            .into_iter()
            .filter(|info| {
                self.performance
                    .as_ref()
                    .and_then(|p| p.config.probe.as_ref())
                    .is_none_or(|serial| info.serial.as_ref() == Some(serial))
            })
            .collect::<Vec<_>>();
        for d in self
            .devices
            .iter_mut()
            .filter(|d| !d.id.starts_with("remote-"))
        {
            d.online = false;
        }
        for info in found {
            let id = info
                .serial
                .clone()
                .unwrap_or_else(|| format!("usb-{}-{}", info.bus, info.address));
            let index = if let Some(index) = self.devices.iter().position(|d| d.id == id) {
                index
            } else {
                let mut d = Device::base(id, info.product);
                d.page = self.initial_page;
                d.swd_tab = self.initial_swd_tab;
                self.devices.push(d);
                self.devices.len() - 1
            };
            let d = &mut self.devices[index];
            d.online = true;
            d.debugger.config.serial = info.serial.unwrap_or_default();
            if let Some(error) = info.access_error {
                d.live_error = format!("USB: {error}");
            } else if d.debugger.config.serial.is_empty() {
                d.live_error = "USB serial unavailable; reconnect the probe".into();
            }
        }
        for d in self.devices.iter_mut().filter(|d| !d.online) {
            d.stop_live(self.start.elapsed().as_secs_f64());
        }
        if !self.devices.get(self.selected).is_some_and(|d| d.online) {
            if let Some(index) = self.devices.iter().position(|d| d.online) {
                self.selected = index;
            }
        }
    }
    fn poll(&mut self, now: f64) {
        if let Some(result) = self.scan.as_ref().map(|rx| rx.try_recv()) {
            match result {
                Ok(Ok(found)) => {
                    self.scan = None;
                    self.apply_discovery(found);
                }
                Ok(Err(e)) => {
                    self.scan = None;
                    self.notice = format!("USB: {e}");
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan = None;
                    self.notice = "USB discovery stopped".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !cfg!(test) && self.discovery_enabled() && self.scan.is_none() && now >= self.next_scan {
            self.scan();
        }
        for d in &mut self.devices {
            d.tick(now);
            d.audit.clear();
        }
    }
    fn open_history(&mut self) {
        if let Some(path) = file_pick(&["jsonl"]) {
            match self
                .history_viewer
                .monitor
                .load(std::path::Path::new(&path))
            {
                Ok(()) => self.show_history = true,
                Err(e) => self.notice = e,
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(feature = "ui-perf")]
        let update_start = Instant::now();
        let now = self.start.elapsed().as_secs_f64();
        #[cfg(feature = "ui-perf")]
        if let Some(runtime) = &mut self.performance {
            runtime.inject();
        }
        self.poll(now);
        self.page_shortcuts(ctx);
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let paths: Vec<_> = dropped
                .iter()
                .filter_map(|file| file.path.clone())
                .collect();
            self.notice = if paths.len() != dropped.len() {
                self.locale
                    .tr("Dropped files must have a local path")
                    .into()
            } else {
                match self
                    .devices
                    .get_mut(self.selected)
                    .ok_or_else(|| "Connect a device before programming".to_string())
                    .and_then(|d| d.accept_file_drop(&paths, now))
                {
                    Ok(message) => self.locale.tr(message).into(),
                    Err(error) => self.locale.tr(&error).into(),
                }
            };
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        if now >= 1.5 && self.screenshot.is_some() && !self.screenshot_requested {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                if let Some(path) = self.screenshot.take() {
                    let bytes: Vec<_> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    match image::save_buffer(
                        &path,
                        &bytes,
                        image.width() as u32,
                        image.height() as u32,
                        image::ColorType::Rgba8,
                    ) {
                        Ok(_) => eprintln!("Screenshot: {path}"),
                        Err(e) => eprintln!("Screenshot failed: {e}"),
                    }
                }
            }
        }
        if self
            .quit_at
            .is_some_and(|t| now > t && (self.screenshot.is_none() || now > t + 3.))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::TopBottomPanel::top("app_header")
            .exact_height(38.)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ctx, |ui| {
                theme::row(ui, |ui| {
                    if theme::icon_toggle(
                        ui,
                        theme::Icon::Devices,
                        self.locale.tr("Devices"),
                        self.sidebar_open,
                        true,
                    )
                    .on_hover_text(self.locale.tr("Show or hide device switcher"))
                    .clicked()
                    {
                        self.sidebar_open = !self.sidebar_open;
                    }
                    ui.separator();
                    if let Some(d) = self.devices.get(self.selected) {
                        ui.label(RichText::new(&d.name).strong());
                        ui.label(RichText::new(&d.id).monospace().size(11.).color(MUTED));
                    } else {
                        ui.label(RichText::new("PicoHarness").strong());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            theme::Icon::Settings,
                            self.locale.tr("Settings"),
                            true,
                        )
                        .clicked()
                        {
                            self.settings = true;
                        }
                        if theme::icon_button(
                            ui,
                            theme::Icon::Scan,
                            self.locale.tr("Scan devices"),
                            self.scan.is_none(),
                        )
                        .clicked()
                        {
                            self.scan();
                        }
                        ui.separator();
                        if let Some(d) = self.devices.get_mut(self.selected) {
                            if theme::icon_button(
                                ui,
                                theme::Icon::Refresh,
                                self.locale.tr("Reset"),
                                d.available(),
                            )
                            .clicked()
                            {
                                if let Err(e) = d.request_live(live::Command::Reset) {
                                    self.notice = e;
                                }
                            }
                            if theme::icon_toggle(
                                ui,
                                theme::Icon::Power,
                                self.locale
                                    .tr(if d.power { "Power off" } else { "Power on" }),
                                d.power,
                                d.live_ready && d.flash.job.is_none(),
                            )
                            .clicked()
                            {
                                if let Err(e) = d.request_live(live::Command::Power(!d.power)) {
                                    self.notice = e;
                                }
                            }
                        }
                    });
                });
            });
        egui::TopBottomPanel::top("page_tabs")
            .exact_height(34.)
            .frame(
                egui::Frame::new()
                    .fill(theme::BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.show_history {
                        if theme::button(ui, self.locale.tr("Back to device"), 88., true).clicked()
                        {
                            self.show_history = false;
                        }
                        ui.separator();
                        ui.label(self.locale.tr("Saved history"));
                    } else if let Some(d) = self.devices.get_mut(self.selected) {
                        for (index, page) in Page::ALL.into_iter().enumerate() {
                            let label = format!("{} [{}]", self.locale.tr(page.name()), index + 1);
                            if theme::tab(
                                ui,
                                &label,
                                if page == Page::Logic { 108. } else { 92. },
                                d.page == page,
                            )
                            .on_hover_text(self.locale.tr(page.name()))
                            .clicked()
                            {
                                d.page = page;
                            }
                        }
                    }
                });
            });
        egui::TopBottomPanel::bottom("status")
            .exact_height(24.)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().interact_size.y = 18.;
                ui.horizontal(|ui| {
                    if let Some(d) = self.devices.get(self.selected) {
                        badge(
                            ui,
                            self.locale.tr(d.state()),
                            if d.live_ready { ACCENT } else { MUTED },
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "RX {:.1} KB   TX {:.1} KB",
                                d.rx as f64 / 1000.,
                                d.tx as f64 / 1000.
                            ))
                            .monospace()
                            .small()
                            .color(MUTED),
                        );
                        if d.monitor.recording() {
                            badge(ui, "REC", RED);
                        }
                    } else {
                        ui.label(
                            RichText::new(self.locale.tr("Connect your PDBG probe to begin"))
                                .small()
                                .color(MUTED),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&self.notice).color(MUTED).size(11.));
                    });
                });
            });
        if self.sidebar_open {
            egui::SidePanel::left("devices")
                .default_width(250.)
                .width_range(220. ..=330.)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(8))
                .show(ctx, |ui| {
                    theme::row(ui, |ui| {
                        ui.label(RichText::new(self.locale.tr("Devices")).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(
                                ui,
                                theme::Icon::Close,
                                self.locale.tr("Close"),
                                true,
                            )
                            .clicked()
                            {
                                self.sidebar_open = false;
                            }
                        });
                    });
                    ui.separator();
                    if self.devices.is_empty() {
                        ui.label(RichText::new(self.locale.tr("No devices found")).color(MUTED));
                    }
                    for (index, d) in self.devices.iter().enumerate() {
                        let response = egui::Frame::new()
                            .fill(if index == self.selected {
                                Color32::from_rgb(52, 62, 72)
                            } else {
                                PANEL
                            })
                            .inner_margin(8)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(&d.name).strong());
                                ui.label(RichText::new(&d.id).monospace().size(10.).color(MUTED));
                                badge(
                                    ui,
                                    self.locale.tr(d.state()),
                                    if d.live_ready { ACCENT } else { MUTED },
                                );
                            })
                            .response;
                        if ui
                            .interact(
                                response.rect,
                                response.id.with("select"),
                                egui::Sense::click(),
                            )
                            .clicked()
                        {
                            self.selected = index;
                            self.show_history = false;
                        }
                        ui.add_space(2.);
                    }
                    ui.add_space(8.);
                    ui.label(
                        RichText::new(self.locale.tr("Devices are detected automatically"))
                            .small()
                            .color(MUTED),
                    );
                });
        }
        let displayed = self
            .devices
            .get(self.selected)
            .filter(|_| !self.show_history)
            .map(|device| (device.id.clone(), device.page));
        if self.displayed_page != displayed {
            ctx.memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
                if let Some(device) = self.devices.get(self.selected) {
                    if !self.show_history && device.page == Page::Console {
                        memory.request_focus(pages::console_input_id(device));
                    }
                }
            });
            self.displayed_page = displayed;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND).inner_margin(8))
            .show(ctx, |ui| {
                if self.show_history {
                    monitor_ui::show(
                        ui,
                        &mut self.history_viewer,
                        &self.locale,
                        now,
                        &mut self.notice,
                    );
                    return;
                }
                let Some(d) = self.devices.get_mut(self.selected) else {
                    ui.add_space((ui.available_height() * 0.23).max(30.));
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(self.locale.tr("Connect your PDBG probe to begin"))
                                .size(17.)
                                .strong(),
                        );

                        ui.add_space(18.);
                        if ui.button(self.locale.tr("Open history")).clicked() {
                            self.open_history();
                        }
                        if self.scan.is_some() {
                            ui.spinner();
                        }
                    });
                    return;
                };
                ui.push_id(d.id.clone(), |ui| match d.page {
                    Page::Gdb => gdb_ui::show(ui, d, &self.locale),
                    Page::Logic => logic_ui::show(ui, d, &self.locale, now, &mut self.notice),
                    _ => pages::show(ui, d, &self.locale, now, &mut self.notice),
                });
            });
        if ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("firmware-drop-overlay"),
            ));
            let rect = ctx.screen_rect();
            painter.rect_filled(rect, 0, Color32::from_black_alpha(210));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                self.locale.tr(if self.devices.is_empty() {
                    "Connect a device before programming"
                } else if self
                    .devices
                    .get(self.selected)
                    .is_some_and(|device| device.page != Page::Firmware)
                    && ctx.input(|input| {
                        input.raw.hovered_files.iter().any(|file| {
                            file.path
                                .as_ref()
                                .and_then(|path| path.extension())
                                .and_then(|extension| extension.to_str())
                                .is_some_and(|extension| {
                                    extension.eq_ignore_ascii_case("elf")
                                        || extension.eq_ignore_ascii_case("axf")
                                })
                        })
                    })
                {
                    "Drop ELF to start GDB debugging"
                } else {
                    "Drop UF2 / ELF / BIN to program"
                }),
                egui::FontId::proportional(28.),
                ACCENT,
            );
        }
        if self.settings {
            let mut add_remote = false;
            let mut open_history = false;
            egui::Window::new(self.locale.tr("Settings"))
                .open(&mut self.settings)
                .default_width(460.)
                .show(ctx, |ui| {
                    ui.label(self.locale.tr("Language"));
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(&self.locale.languages[self.locale.selected].name)
                        .show_ui(ui, |ui| {
                            for (i, language) in self.locale.languages.iter().enumerate() {
                                ui.selectable_value(&mut self.locale.selected, i, &language.name);
                            }
                        });
                    if let Some(d) = self.devices.get_mut(self.selected) {
                        ui.add_space(10.);
                        ui.label(self.locale.tr("Device name"));
                        crate::theme::text_field(ui, &mut d.name);
                        if ui.button(self.locale.tr("Reconnect")).clicked() {
                            d.stop_live(now);
                            d.live_retry_at = 0.;
                        }
                        if !d.live_info.is_empty() {
                            ui.label(RichText::new(&d.live_info).small().color(MUTED));
                        }
                        ui.separator();
                        ui.label(self.locale.tr("Debug Core firmware"));
                        ui.horizontal(|ui| {
                            crate::theme::text_field_sized(
                                ui,
                                &mut d.debug_image,
                                (ui.available_width() - 88.).max(80.),
                            );
                            if ui
                                .add_enabled(
                                    d.debug_update_job.is_none(),
                                    egui::Button::new(self.locale.tr("Browse")),
                                )
                                .clicked()
                            {
                                if let Some(path) = file_pick(&["uf2"]) {
                                    d.debug_image = path;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("picotool");
                            crate::theme::text_field(ui, &mut d.picotool);
                        });
                        let idle = d.online
                            && d.debug_update_job.is_none()
                            && d.flash.job.is_none()
                            && d.debugger.session.is_none()
                            && !d.swd
                            && !d.swd_pending
                            && !d.debugger.config.serial.is_empty()
                            && !d.task.as_ref().is_some_and(|task| task.status == "Running");
                        if ui
                            .add_enabled(
                                idle && !d.debug_image.is_empty(),
                                egui::Button::new(self.locale.tr("Upgrade Debug Core")),
                            )
                            .clicked()
                        {
                            d.stop_live(now);
                            d.debug_update_status = self.locale.tr("Validating").into();
                            d.debug_update_job =
                                Some(ph_app::debug_update::Job::start(
                                    d.debugger.config.serial.clone(),
                                    d.debug_image.clone().into(),
                                    d.picotool.clone(),
                                ));
                        }
                        if !d.debug_update_status.is_empty() {
                            ui.label(&d.debug_update_status);
                        }
                    }
                    ui.separator();
                    ui.label(self.locale.tr("Recordings are saved automatically"));
                    ui.label(
                        RichText::new(
                            ph_app::telemetry::recording_dir()
                                .display()
                                .to_string(),
                        )
                        .small()
                        .color(MUTED),
                    );
                    open_history = ui.button(self.locale.tr("Open history")).clicked();
                    egui::CollapsingHeader::new(self.locale.tr("Advanced")).show(ui, |ui| {
                        add_remote = ui.button(self.locale.tr("Add OpenOCD session")).clicked();
                    });
                });
            if add_remote {
                let mut d =
                    Device::base(format!("remote-{}", self.devices.len()), "OpenOCD".into());
                d.online = true;
                d.page = Page::Gdb;
                d.debugger.config.managed = false;
                self.devices.push(d);
                self.selected = self.devices.len() - 1;
            }
            if open_history {
                self.open_history();
            }
        }
        #[cfg(feature = "ui-perf")]
        if let Some(runtime) = &mut self.performance {
            runtime.after_update(ctx, _frame.info().cpu_usage, update_start, &self.devices);
        }
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(feature = "ui-perf")]
    let performance = performance::Config::parse(&std::env::args().collect::<Vec<_>>())
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360., 940.])
            .with_min_inner_size([960., 700.]),
        ..Default::default()
    };
    #[cfg(feature = "ui-perf")]
    if let Some(performance) = &performance {
        options.persist_window = false;
        options.persistence_path = Some(performance.persistence_path());
        options.renderer = eframe::Renderer::Glow;
        // Keep the benchmark geometry independent of saved desktop preferences.
        options.viewport = options.viewport.with_position([40., 40.]);
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        // winit 0.30's Wayland backend does not deliver file-drop events.
        // XWayland supports XDND while keeping the app usable on Wayland desktops.
        use winit::platform::x11::EventLoopBuilderExtX11;
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }
    eframe::run_native(
        "PicoHarness",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> (egui::Context, App) {
        let ctx = egui::Context::default();
        let app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        (ctx, app)
    }
    fn info(serial: &str) -> ph_app_link::DeviceInfo {
        ph_app_link::DeviceInfo {
            serial: Some(serial.into()),
            bus: 1,
            address: 2,
            product: "PDBG Probe".into(),
            access_error: None,
        }
    }
    #[test]
    fn startup_contains_only_discovered_devices() {
        let (_, app) = app();
        assert!(
            app.devices.is_empty(),
            "Startup must not invent connected demo devices"
        );
        assert!(!app.sidebar_open);
    }

    #[test]
    fn application_toolbars_keep_controls_aligned_with_sidebar_open_or_closed() {
        for width in [960., 1360.] {
            for language in [0, 1] {
                for sidebar in [false, true] {
                    let (ctx, mut app) = app();
                    app.locale.selected = language;
                    app.sidebar_open = sidebar;
                    app.devices.push(Device::base(
                        "test-serial".into(),
                        "PDBG Pico (CMSIS-DAP)".into(),
                    ));
                    let mut frame = eframe::Frame::_new_kittest();
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 700.),
                            )),
                            ..Default::default()
                        },
                        |ctx| eframe::App::update(&mut app, ctx, &mut frame),
                    );
                    let controls = ctx
                        .data(|data| {
                            data.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new(
                                "toolbar_geometry",
                            ))
                        })
                        .unwrap();
                    assert!(controls.len() >= 10);
                    for (index, (name, rect)) in controls.iter().enumerate() {
                        assert!(
                            (rect.height() - theme::CONTROL_HEIGHT).abs() < 0.1,
                            "{name} has inconsistent height: {rect:?}"
                        );
                        assert!(
                            rect.left() >= 0. && rect.right() <= width,
                            "{name} is outside the window: {rect:?}"
                        );
                        for (_, other) in &controls[index + 1..] {
                            assert!(
                                !rect.intersects(*other),
                                "controls overlap: {rect:?}, {other:?}"
                            );
                            if (rect.center().y - other.center().y).abs() < theme::CONTROL_HEIGHT {
                                assert!(
                                    (rect.center().y - other.center().y).abs() < 0.1,
                                    "controls in the same toolbar have different baselines"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn discovery_selects_connected_device_and_preserves_state_on_reconnect() {
        let (_, mut app) = app();
        app.apply_discovery(vec![info("A")]);
        assert_eq!(app.devices[app.selected].id, "A");
        assert!(app.devices[0].page == Page::Overview);
        app.devices[0].name = "My probe".into();
        app.apply_discovery(vec![]);
        assert!(!app.devices[0].online);
        app.apply_discovery(vec![info("B")]);
        assert_eq!(app.devices[app.selected].id, "B");
        app.apply_discovery(vec![info("A")]);
        assert_eq!(app.devices.len(), 2);
        assert_eq!(app.devices[app.selected].name, "My probe");
    }
    #[test]
    fn empty_workspace_and_firmware_drop_render_without_panicking() {
        let (ctx, mut app) = app();
        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960., 700.),
                )),
                dropped_files: vec![egui::DroppedFile {
                    path: Some("image.uf2".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            |ctx| eframe::App::update(&mut app, ctx, &mut frame),
        );
        assert_eq!(app.notice, "Connect a device before programming");
        app.apply_discovery(vec![info("test")]);
        app.devices[0].online = false;
        app.devices[0].page = Page::Firmware;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame)
        });
        assert!(app.devices[0].flash.job.is_none());
    }

    #[test]
    #[ignore = "Requires connected PDBG USB probe; records real measurements in /tmp"]
    fn connected_probe_records_and_exports_real_measurements() {
        use ph_app::telemetry::History;
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("Set PDBG_PROBE_SERIAL");
        let mut link = ph_app_link::PhBoardLink::open_serial(&serial).unwrap();
        let before = link.status().unwrap();
        println!("Probe {serial}: {}", link.info().unwrap());
        println!("Initial status: {before:?}");
        drop(link);
        struct RestorePower(String, bool);
        impl Drop for RestorePower {
            fn drop(&mut self) {
                if let Ok(mut link) = ph_app_link::PhBoardLink::open_serial(&self.0) {
                    if link.status().is_ok_and(|s| s.power_on != self.1) {
                        let _ = link.set_power(self.1);
                    }
                }
            }
        }
        let restore = RestorePower(serial.clone(), before.power_on);
        let (ctx, mut app) = app();
        let found = ph_app_link::enumerate_devices().unwrap();
        assert!(found.iter().any(|d| d.serial.as_deref() == Some(&serial)));
        app.apply_discovery(found);
        app.selected = app.devices.iter().position(|d| d.id == serial).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !app.devices[app.selected].live_ready {
            assert!(Instant::now() < deadline, "USB worker did not become ready");
            app.poll(app.start.elapsed().as_secs_f64());
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!app.sidebar_open);
        let dir = std::env::temp_dir().join("ph-app-hardware-check");
        app.devices[app.selected]
            .monitor
            .start(&serial, &dir)
            .unwrap();
        // Exercise actual Work power and sensor readings, then restore the initial state.
        app.devices[app.selected]
            .request_live(live::Command::Power(true))
            .unwrap();
        while app.devices[app.selected]
            .monitor
            .session
            .as_ref()
            .unwrap()
            .samples
            .len()
            < 12
        {
            assert!(Instant::now() < deadline, "Real telemetry did not arrive");
            app.poll(app.start.elapsed().as_secs_f64());
            std::thread::sleep(Duration::from_millis(25));
        }
        let d = &mut app.devices[app.selected];
        d.monitor.stop().unwrap();
        let path = d.monitor.session.as_ref().unwrap().path.clone().unwrap();
        let saved = History::read(&path).unwrap();
        assert_eq!(saved.samples.len(), 12);
        assert!(saved.samples.iter().all(|s| s.bus_voltage_v.is_finite()));
        assert!(saved
            .samples
            .iter()
            .any(|s| s.power_on && s.bus_voltage_v > 4. && s.work_3v3_v > 3.));
        assert!(saved
            .samples
            .windows(2)
            .all(|p| p[1].timestamp_unix_ms > p[0].timestamp_unix_ms));
        let csv = dir.join("hardware.csv");
        std::fs::write(&csv, saved.csv()).unwrap();
        assert_eq!(std::fs::read_to_string(&csv).unwrap().lines().count(), 13);
        d.monitor.load(&path).unwrap();
        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1360., 940.),
                )),
                ..Default::default()
            },
            |ctx| eframe::App::update(&mut app, ctx, &mut frame),
        );
        println!(
            "Recorded {} real samples: {:?}",
            saved.samples.len(),
            saved.samples.last().unwrap()
        );
        println!("History: {}\nCSV: {}", path.display(), csv.display());
        drop(app);
        drop(restore);
        let mut link = ph_app_link::PhBoardLink::open_serial(serial).unwrap();
        assert_eq!(link.status().unwrap().power_on, before.power_on);
    }
}
