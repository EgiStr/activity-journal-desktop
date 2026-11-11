//! The window: status, journals, run log and settings. All state the engine owns is read from its
//! files on each refresh; the app keeps only the run it started and the form being edited.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{Color32, RichText};

use crate::desktop_config;
use crate::engine_run::{self, RunKind, RunMessage};
use crate::exit_code::{Outcome, Severity, outcome_of};
use crate::journal_files::{DayJournals, find_journals};
use crate::lifecycle::{DayRecord, lock_owner, read_state};
use crate::settings::{Layers, PluginEntry, SettingsForm, discover_plugins, resolve};
use crate::tray::TrayState;

const REFRESH_EVERY: Duration = Duration::from_secs(15);
const LOG_LIMIT: usize = 5_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Journal,
    Run,
    Settings,
}

/// What the engine's files say, re-read on every refresh.
#[derive(Default)]
struct EngineView {
    days: Vec<DayRecord>,
    journals: BTreeMap<String, DayJournals>,
    plugins: Vec<PluginEntry>,
    staged: Vec<String>,
    lock_pid: Option<u32>,
    errors: Vec<String>,
}

struct RunState {
    kind: RunKind,
    started: Instant,
    receiver: Receiver<RunMessage>,
}

pub struct App {
    engine_root: Option<PathBuf>,
    root_input: String,
    tab: Tab,
    view: EngineView,
    refreshed: Instant,
    settings: Option<Layers>,
    pipeline: Option<Layers>,
    form: SettingsForm,
    plugin_toggles: BTreeMap<String, bool>,
    save_message: Option<(bool, String)>,
    autostart: bool,
    run: Option<RunState>,
    last_run: Option<(RunKind, Option<Outcome>)>,
    log: Vec<String>,
    selected_day: Option<String>,
    show_checkpoint: bool,
    journal_text: String,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl App {
    pub fn new(engine_root: Option<PathBuf>, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut app = Self {
            root_input: engine_root
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            engine_root,
            tab: Tab::Status,
            view: EngineView::default(),
            refreshed: Instant::now(),
            settings: None,
            pipeline: None,
            form: SettingsForm::default(),
            plugin_toggles: BTreeMap::new(),
            save_message: None,
            autostart: desktop_config::autostart_enabled(),
            run: None,
            last_run: None,
            log: Vec::new(),
            selected_day: None,
            show_checkpoint: false,
            journal_text: String::new(),
            wake,
        };
        app.refresh();
        app.load_form();
        app
    }

    /// Re-read everything the engine owns.
    pub fn refresh(&mut self) {
        self.refreshed = Instant::now();
        let Some(root) = self.engine_root.clone() else {
            self.view = EngineView {
                errors: vec!["Choose the Activity Journal folder in Settings.".into()],
                ..Default::default()
            };
            return;
        };
        let mut view = EngineView::default();
        match Layers::load(&root, "settings") {
            Ok(settings) => {
                let data_dir = resolve(
                    &root,
                    &settings
                        .get_str(&["paths", "data_dir"])
                        .unwrap_or_else(|| "./data".into()),
                );
                let journal_dir = resolve(
                    &root,
                    &settings
                        .get_str(&["paths", "journal_dir"])
                        .unwrap_or_else(|| "./journal".into()),
                );
                match read_state(&data_dir.join("state.json")) {
                    Ok(days) => view.days = days,
                    Err(e) => view.errors.push(e),
                }
                view.lock_pid = lock_owner(&data_dir);
                view.journals = find_journals(&journal_dir);
                if view.journals.is_empty() {
                    view.errors
                        .push(format!("No journals under {}.", journal_dir.join("journal").display()));
                }
            }
            Err(e) => view.errors.push(e),
        }
        match Layers::load(&root, "pipeline") {
            Ok(pipeline) => view.staged = pipeline.staged_plugins(),
            Err(e) => view.errors.push(e),
        }
        view.plugins = discover_plugins(&root);
        self.view = view;
        if let Some(day) = self.selected_day.clone() {
            self.open_journal(&day);
        }
    }

    fn load_form(&mut self) {
        let Some(root) = self.engine_root.clone() else { return };
        self.settings = Layers::load(&root, "settings").ok();
        self.pipeline = Layers::load(&root, "pipeline").ok();
        self.form = self
            .settings
            .as_ref()
            .map(SettingsForm::from_layers)
            .unwrap_or_default();
        self.plugin_toggles = self
            .view
            .plugins
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    self.pipeline.as_ref().is_none_or(|l| l.plugin_enabled(&p.name)),
                )
            })
            .collect();
    }

    /// Called on every loop turn: drain the run's output and refresh on a timer.
    pub fn tick(&mut self) -> Option<Instant> {
        let mut finished = None;
        if let Some(run) = &self.run {
            while let Ok(message) = run.receiver.try_recv() {
                match message {
                    RunMessage::Line(line) => {
                        self.log.push(line);
                        if self.log.len() > LOG_LIMIT {
                            self.log.drain(..self.log.len() - LOG_LIMIT);
                        }
                    }
                    RunMessage::Finished(code) => finished = Some((run.kind, code)),
                }
            }
        }
        if let Some((kind, code)) = finished {
            let outcome = code.map(outcome_of);
            self.log.push(match outcome {
                Some(o) => format!("— exit {} ({}): {}", o.code, o.label, o.advice),
                None => "— the engine did not report an exit code".to_owned(),
            });
            self.last_run = Some((kind, outcome));
            self.run = None;
            self.refresh();
        } else if self.refreshed.elapsed() >= REFRESH_EVERY {
            self.refresh();
        }
        Some(self.refreshed + REFRESH_EVERY)
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Start a run unless one is already going. The engine's own lock still decides across processes.
    pub fn start_run(&mut self, kind: RunKind) {
        if self.run.is_some() {
            return;
        }
        let Some(root) = self.engine_root.clone() else {
            self.tab = Tab::Settings;
            return;
        };
        let (sender, receiver): (Sender<RunMessage>, _) = channel();
        self.log
            .push(format!("— {} started: uv {}", kind.label(), kind.arguments().join(" ")));
        let wake = self.wake.clone();
        engine_run::start(&root, kind, sender, move || wake());
        self.run = Some(RunState {
            kind,
            started: Instant::now(),
            receiver,
        });
        self.tab = Tab::Run;
    }

    pub fn tray_state(&self) -> TrayState {
        if self.run.is_some() {
            return TrayState::Running;
        }
        match self.last_run {
            Some((_, None)) => return TrayState::Blocked,
            Some((_, Some(o))) if o.severity == Severity::Blocked => return TrayState::Blocked,
            Some((_, Some(o))) if o.severity == Severity::Caveat => return TrayState::Partial,
            _ => {}
        }
        if self.view.days.iter().any(|d| d.stuck_cause.is_some()) {
            TrayState::Blocked
        } else {
            TrayState::Idle
        }
    }

    fn open_journal(&mut self, day: &str) {
        self.selected_day = Some(day.to_owned());
        let files = self.view.journals.get(day).cloned().unwrap_or_default();
        let path = if self.show_checkpoint {
            files.checkpoint.or(files.final_journal)
        } else {
            files.final_journal.or(files.checkpoint)
        };
        self.journal_text = match path {
            Some(path) => {
                std::fs::read_to_string(&path).unwrap_or_else(|e| format!("Cannot read {}: {e}", path.display()))
            }
            None => "No journal file for this day.".to_owned(),
        };
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.tab, Tab::Journal, "Journal");
                ui.selectable_value(&mut self.tab, Tab::Run, "Run");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.separator();
                let running = self.run.is_some();
                if ui.add_enabled(!running, egui::Button::new("Run checkpoint")).clicked() {
                    self.start_run(RunKind::Checkpoint);
                }
                if ui.add_enabled(!running, egui::Button::new("Run final")).clicked() {
                    self.start_run(RunKind::Final);
                }
                if ui.add_enabled(!running, egui::Button::new("Dry run")).clicked() {
                    self.start_run(RunKind::DryRun);
                }
                if ui.button("Refresh").clicked() {
                    self.refresh();
                }
            });
        });
        egui::CentralPanel::default_margins().show(ui, |ui| match self.tab {
            Tab::Status => self.status_ui(ui),
            Tab::Journal => self.journal_ui(ui),
            Tab::Run => self.run_ui(ui),
            Tab::Settings => self.settings_ui(ui),
        });
    }

    fn status_ui(&mut self, ui: &mut egui::Ui) {
        let state = self.tray_state();
        ui.heading(state.tooltip().trim_start_matches("Activity Journal: "));
        for error in &self.view.errors {
            ui.colored_label(Color32::from_rgb(215, 58, 73), error);
        }
        if let Some(run) = &self.run {
            ui.label(format!(
                "{} running for {}s.",
                run.kind.label(),
                run.started.elapsed().as_secs()
            ));
        } else if let Some(pid) = self.view.lock_pid {
            ui.label(format!(
                "The engine lock names process {pid}; a scheduled run may be in progress."
            ));
        }
        if let Some((kind, outcome)) = &self.last_run {
            match outcome {
                Some(o) => ui.label(
                    RichText::new(format!(
                        "Last {}: exit {} — {}. {}",
                        kind.label().to_lowercase(),
                        o.code,
                        o.label,
                        o.advice
                    ))
                    .color(severity_colour(o.severity)),
                ),
                None => ui.colored_label(
                    severity_colour(Severity::Blocked),
                    "Last run: the engine could not be started.",
                ),
            };
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Days in state.json").strong());
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("days").striped(true).num_columns(5).show(ui, |ui| {
                for header in ["Day", "Status", "Since", "Stuck", "Journal"] {
                    ui.label(RichText::new(header).strong());
                }
                ui.end_row();
                let mut open = None;
                for day in &self.view.days {
                    ui.label(&day.day_id);
                    ui.label(&day.status);
                    ui.label(&day.since);
                    match &day.stuck_cause {
                        Some(cause) => ui.colored_label(severity_colour(Severity::Blocked), cause),
                        None => ui.label(""),
                    };
                    if self.view.journals.contains_key(&day.day_id) && ui.small_button("Open").clicked() {
                        open = Some(day.day_id.clone());
                    }
                    ui.end_row();
                }
                if let Some(day) = open {
                    self.open_journal(&day);
                    self.tab = Tab::Journal;
                }
            });
            if self.view.days.is_empty() {
                ui.label("No day has started yet.");
            }
        });
    }

    fn journal_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("days")
            .resizable(true)
            .default_size(150.0)
            .show(ui, |ui| {
                if ui.checkbox(&mut self.show_checkpoint, "Prefer checkpoint").changed()
                    && let Some(day) = self.selected_day.clone()
                {
                    self.open_journal(&day);
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let days: Vec<(String, DayJournals)> = self
                        .view
                        .journals
                        .iter()
                        .rev()
                        .map(|(d, j)| (d.clone(), j.clone()))
                        .collect();
                    for (day, files) in days {
                        let marker = if files.final_journal.is_some() {
                            ""
                        } else {
                            " (checkpoint)"
                        };
                        let selected = self.selected_day.as_deref() == Some(day.as_str());
                        if ui.selectable_label(selected, format!("{day}{marker}")).clicked() {
                            self.open_journal(&day);
                        }
                    }
                });
            });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.selected_day.is_none() {
                    ui.label("Pick a day on the left.");
                }
                markdown_lines(ui, &self.journal_text);
            });
        });
    }

    fn run_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Clear log").clicked() {
                self.log.clear();
            }
            if let Some(run) = &self.run {
                ui.spinner();
                ui.label(format!("{} running…", run.kind.label()));
            }
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &self.log {
                    ui.monospace(line);
                }
            });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Activity Journal folder").strong());
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.root_input);
                if ui.button("Use").clicked() {
                    let path = PathBuf::from(self.root_input.trim());
                    if desktop_config::is_engine_root(&path) {
                        let saved = desktop_config::save_engine_root(&path);
                        self.engine_root = Some(path);
                        self.refresh();
                        self.load_form();
                        self.save_message = Some(match saved {
                            Ok(()) => (true, "Engine folder saved.".to_owned()),
                            Err(e) => (false, e),
                        });
                    } else {
                        self.save_message = Some((
                            false,
                            "That folder has no pyproject.toml and src/activity_journal.".to_owned(),
                        ));
                    }
                }
            });
            if ui
                .checkbox(&mut self.autostart, "Start this app at logon (tray only)")
                .changed()
                && let Err(e) = desktop_config::set_autostart(self.autostart)
            {
                self.autostart = desktop_config::autostart_enabled();
                self.save_message = Some((false, format!("Autostart: {e}")));
            }
            ui.label("Scheduled runs stay in Task Scheduler, so they happen even when this app is closed.");
            ui.separator();

            let Some(root) = self.engine_root.clone() else { return };
            ui.label(RichText::new("config/settings.local.toml").strong());
            egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                text_row(ui, "Data directory", &mut self.form.data_dir);
                text_row(ui, "Journal directory", &mut self.form.journal_dir);
                text_row(ui, "Timezone", &mut self.form.timezone);
                text_row(ui, "Model", &mut self.form.model);
                text_row(ui, "Router URL", &mut self.form.base_url);
                ui.label("Narrative via router");
                ui.checkbox(&mut self.form.router_enabled, "");
                ui.end_row();
                ui.label("Privacy gate");
                ui.checkbox(&mut self.form.privacy_enabled, "");
                ui.end_row();
            });
            ui.label("Secrets (ROUTER_API_KEY, BRAIN_TOKEN) stay in the environment and are never shown here.");
            ui.separator();

            ui.label(RichText::new("Plugins (config/pipeline.local.toml)").strong());
            egui::Grid::new("plugins").striped(true).num_columns(3).show(ui, |ui| {
                for plugin in &self.view.plugins {
                    let enabled = self.plugin_toggles.entry(plugin.name.clone()).or_insert(true);
                    ui.checkbox(enabled, &plugin.name);
                    ui.label(&plugin.kind);
                    if self.view.staged.contains(&plugin.name) {
                        ui.label(&plugin.description);
                    } else {
                        ui.label(RichText::new("not in any stage; never runs").weak());
                    }
                    ui.end_row();
                }
            });
            ui.separator();

            let problems = self.form.problems(&root);
            for problem in &problems {
                ui.colored_label(severity_colour(Severity::Blocked), problem);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(problems.is_empty(), egui::Button::new("Save")).clicked() {
                    self.save_message = Some(self.save(&root));
                }
                if ui.button("Revert").clicked() {
                    self.load_form();
                    self.save_message = None;
                }
            });
            if let Some((ok, message)) = &self.save_message {
                let colour = if *ok {
                    severity_colour(Severity::Ok)
                } else {
                    severity_colour(Severity::Blocked)
                };
                ui.colored_label(colour, message);
            }
        });
    }

    fn save(&mut self, root: &Path) -> (bool, String) {
        let result = (|| {
            let mut settings = Layers::load(root, "settings")?;
            self.form.apply(&mut settings);
            settings.save()?;
            let mut pipeline = Layers::load(root, "pipeline")?;
            for (name, enabled) in &self.plugin_toggles {
                pipeline.set_bool(&["plugins", name, "enabled"], *enabled);
            }
            pipeline.save()
        })();
        self.refresh();
        self.load_form();
        match result {
            Ok(()) => (true, "Saved. The next run uses these settings.".to_owned()),
            Err(e) => (false, e),
        }
    }
}

fn text_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(380.0));
    ui.end_row();
}

fn severity_colour(severity: Severity) -> Color32 {
    match severity {
        Severity::Ok => Color32::from_rgb(46, 160, 67),
        Severity::Caveat => Color32::from_rgb(222, 160, 30),
        Severity::Blocked => Color32::from_rgb(215, 58, 73),
    }
}

/// Show Markdown read-only, with headings emphasised. Nothing is parsed for numbers.
fn markdown_lines(ui: &mut egui::Ui, text: &str) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if level > 0 && trimmed[level..].starts_with(' ') {
            let size = match level {
                1 => 22.0,
                2 => 18.0,
                _ => 15.0,
            };
            ui.add_space(4.0);
            ui.label(RichText::new(trimmed[level..].trim()).size(size).strong());
        } else if line.trim().is_empty() {
            ui.add_space(4.0);
        } else {
            ui.label(line);
        }
    }
}
