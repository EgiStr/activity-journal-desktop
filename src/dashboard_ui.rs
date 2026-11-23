//! The Dashboard tab (PLAN 2c). History comes from `journal history --json` (read-only, no lock);
//! today comes from `journal timeline --json`, fetched only when the owner asks, because for a day
//! not yet stored it takes the engine's run lock while it collects. A day without stats is shown
//! as missing, never as zeros, and a degraded source is named.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{Color32, RichText};
use egui_plot::{Bar, BarChart, Legend, Plot};

use crate::dashboard_data::{
    Metric, Share, WeekRecap, by_project, by_source, cost_total, duration, hourly, reconciles, week_recap,
};
use crate::engine_json::{DayStats, HistoryDay, Timeline, parse_history, parse_timeline};
use crate::engine_run::{self, QueryResult};
use crate::journal_files::DayJournals;
use crate::journal_search::{self, SearchResult};

const HISTORY_RANGES: [u32; 3] = [7, 14, 30];
const WARN: Color32 = Color32::from_rgb(222, 160, 30);
const BAD: Color32 = Color32::from_rgb(215, 58, 73);

#[derive(Clone, PartialEq, Eq)]
enum Selected {
    Today,
    Week,
    Search,
    Day(String),
}

/// What the tab asks the window to do.
pub enum Action {
    OpenJournal(String),
}

pub struct Dashboard {
    metric: Metric,
    range: u32,
    selected: Selected,
    today: Option<Result<Timeline, String>>,
    today_pending: Option<Receiver<QueryResult>>,
    history: Option<Result<Vec<HistoryDay>, String>>,
    history_pending: Option<Receiver<QueryResult>>,
    query: String,
    search: Option<SearchResult>,
}

impl Default for Dashboard {
    fn default() -> Self {
        Self {
            metric: Metric::Active,
            range: HISTORY_RANGES[0],
            selected: Selected::Today,
            today: None,
            today_pending: None,
            history: None,
            history_pending: None,
            query: String::new(),
            search: None,
        }
    }
}

impl Dashboard {
    /// Forget everything, as after the engine folder changes.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Mark history stale so the next visit reloads it, as after a run.
    pub fn history_changed(&mut self) {
        if self.history_pending.is_none() {
            self.history = None;
        }
    }

    pub fn is_loading(&self) -> bool {
        self.today_pending.is_some() || self.history_pending.is_some()
    }

    /// Collect a result that arrived since the last frame.
    pub fn poll(&mut self) {
        if let Some(result) = self.today_pending.as_ref().and_then(|r| r.try_recv().ok()) {
            self.today_pending = None;
            self.today = Some(parse_timeline(&result.stdout).map_err(|e| failure(e, &result)));
        }
        if let Some(result) = self.history_pending.as_ref().and_then(|r| r.try_recv().ok()) {
            self.history_pending = None;
            self.history = Some(parse_history(&result.stdout).map_err(|e| failure(e, &result)));
        }
    }

    fn load_today(&mut self, root: &Path, wake: &Arc<dyn Fn() + Send + Sync>) {
        let (sender, receiver) = channel();
        let wake = wake.clone();
        engine_run::query(root, &["run", "journal", "timeline", "--json"], sender, move || wake());
        self.today_pending = Some(receiver);
    }

    fn load_history(&mut self, root: &Path, today: &str, wake: &Arc<dyn Fn() + Send + Sync>) {
        let Some(first) = days_before(today, self.range - 1) else {
            self.history = Some(Err(format!("Cannot work out the range before {today}.")));
            return;
        };
        let (sender, receiver) = channel();
        let wake = wake.clone();
        let arguments = ["run", "journal", "history", "--from", &first, "--to", today, "--json"];
        engine_run::query(root, &arguments, sender, move || wake());
        self.history_pending = Some(receiver);
    }

    /// Draw the tab. `today` is the engine's current day id; `run_active` is true while this app
    /// runs the engine, when collecting today would only meet the engine's own lock.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        today: Option<&str>,
        run_active: bool,
        journals: &BTreeMap<String, DayJournals>,
        wake: &Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Action> {
        let (Some(root), Some(today)) = (root, today) else {
            ui.label("Choose the Activity Journal folder in Settings.");
            return None;
        };
        if !engine_run::speaks_json(root) {
            ui.label(
                "This engine has no `journal history` or `journal timeline` yet. Update it to a                  version with the `--json` commands to see the dashboard.",
            );
            return None;
        }
        if self.history.is_none() && self.history_pending.is_none() {
            self.load_history(root, today, wake);
        }
        let mut action = None;
        egui::Panel::left("history")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                self.history_ui(ui, root, today, wake);
            });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                action = self.day_ui(ui, root, today, run_active, journals, wake);
            });
        });
        action
    }

    fn history_ui(&mut self, ui: &mut egui::Ui, root: &Path, today: &str, wake: &Arc<dyn Fn() + Send + Sync>) {
        ui.horizontal(|ui| {
            for days in HISTORY_RANGES {
                if ui
                    .selectable_label(self.range == days, format!("{days} days"))
                    .clicked()
                    && self.range != days
                {
                    self.range = days;
                    self.load_history(root, today, wake);
                }
            }
            if ui
                .add_enabled(self.history_pending.is_none(), egui::Button::new("Reload"))
                .clicked()
            {
                self.load_history(root, today, wake);
            }
        });
        ui.selectable_value(&mut self.selected, Selected::Today, RichText::new("Today").strong());
        ui.selectable_value(&mut self.selected, Selected::Week, RichText::new("This week").strong());
        ui.selectable_value(
            &mut self.selected,
            Selected::Search,
            RichText::new("Search journals").strong(),
        );
        if self.history_pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading stats files…");
            });
        }
        match &self.history {
            Some(Err(e)) => {
                ui.colored_label(BAD, e);
            }
            Some(Ok(days)) => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("history").striped(true).num_columns(3).show(ui, |ui| {
                        for day in days.iter().rev() {
                            let picked = self.selected == Selected::Day(day.day.clone());
                            if ui.selectable_label(picked, &day.day).clicked() {
                                self.selected = Selected::Day(day.day.clone());
                            }
                            match &day.stats {
                                Some(s) => {
                                    ui.label(duration(s.total_active_seconds));
                                    if s.partial {
                                        ui.colored_label(WARN, "partial");
                                    } else {
                                        ui.label(format!("{} events", s.total_events));
                                    }
                                }
                                None => {
                                    ui.label(RichText::new(missing_label(&day.state)).weak());
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
            }
            None => {}
        }
    }

    /// Search the published journals' text, newest day first.
    fn search_ui(&mut self, ui: &mut egui::Ui, journals: &BTreeMap<String, DayJournals>) -> Option<Action> {
        let mut action = None;
        let mut submit = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Every word must be on the same line")
                    .desired_width(320.0),
            );
            submit |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            submit |= ui.button("Search").clicked();
        });
        if submit {
            self.search = Some(journal_search::search(journals, &self.query));
        }
        let Some(result) = &self.search else {
            ui.label(format!(
                "{} day(s) of journals are indexed on disk. Final journals are searched; a day with                  only a checkpoint is searched through its checkpoint.",
                journals.len()
            ));
            return None;
        };
        ui.label(format!(
            "{} match(es) in {} day(s) searched{}",
            result.hits.len(),
            result.days_searched,
            if result.truncated {
                " (list cut short, narrow the query)"
            } else {
                ""
            }
        ));
        if result.unreadable > 0 {
            ui.colored_label(
                WARN,
                format!(
                    "{} journal file(s) could not be read, so they are not covered.",
                    result.unreadable
                ),
            );
        }
        ui.separator();
        egui::Grid::new("search-hits")
            .striped(true)
            .num_columns(3)
            .show(ui, |ui| {
                for hit in &result.hits {
                    let label = if hit.checkpoint {
                        format!("{} (checkpoint)", hit.day)
                    } else {
                        hit.day.clone()
                    };
                    if ui.link(label).clicked() {
                        action = Some(Action::OpenJournal(hit.day.clone()));
                    }
                    ui.label(RichText::new(format!("line {}", hit.line)).weak());
                    ui.label(&hit.text);
                    ui.end_row();
                }
            });
        action
    }

    fn day_ui(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        today: &str,
        run_active: bool,
        journals: &BTreeMap<String, DayJournals>,
        wake: &Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Action> {
        match self.selected.clone() {
            Selected::Today => {
                ui.horizontal(|ui| {
                    let idle = self.today_pending.is_none() && !run_active;
                    if ui.add_enabled(idle, egui::Button::new("Load today")).clicked() {
                        self.load_today(root, wake);
                    }
                    if self.today_pending.is_some() {
                        ui.spinner();
                        ui.label("Collecting today…");
                    }
                });
                ui.label(
                    RichText::new(
                        "Today is not stored before the nightly run, so loading it collects every source \
                         under the engine's run lock (up to two minutes). A scheduled run starting meanwhile \
                         exits 75, so load it on demand, not on a timer.",
                    )
                    .weak(),
                );
                ui.separator();
                match &self.today {
                    None => {
                        ui.label("Not loaded.");
                    }
                    Some(Err(e)) => {
                        ui.colored_label(BAD, e);
                    }
                    Some(Ok(timeline)) => {
                        let origin = match timeline.origin.as_str() {
                            "snapshot" => "collected just now, nothing stored",
                            "stored" => "read from the event store",
                            "published" => "read from its stats file",
                            _ => "not read",
                        };
                        ui.label(format!(
                            "{} — {origin}{}",
                            timeline.day,
                            timeline
                                .status
                                .as_ref()
                                .map(|s| format!(" (status {s})"))
                                .unwrap_or_default()
                        ));
                        if let Some(failure) = &timeline.failure {
                            ui.colored_label(
                                BAD,
                                format!("exit {} ({}): {failure}", timeline.exit_code, timeline.exit_reason),
                            );
                        }
                        match &timeline.stats {
                            Some(stats) => return stats_ui(&mut self.metric, ui, stats),
                            None => {
                                ui.label("No totals for this day. This is missing data, not a day of zeros.");
                            }
                        }
                    }
                }
                None
            }
            Selected::Search => self.search_ui(ui, journals),
            Selected::Week => {
                let recap = self
                    .history
                    .as_ref()
                    .and_then(|h| h.as_ref().ok())
                    .and_then(|days| week_recap(days, today));
                match recap {
                    Some(recap) => week_ui(&mut self.metric, ui, &recap),
                    None => {
                        ui.label("The week is not loaded yet.");
                        None
                    }
                }
            }
            Selected::Day(day) => {
                let entry = self
                    .history
                    .as_ref()
                    .and_then(|h| h.as_ref().ok())
                    .and_then(|days| days.iter().find(|d| d.day == day))
                    .cloned();
                let Some(entry) = entry else {
                    ui.label("That day is not in the loaded range.");
                    return None;
                };
                match &entry.stats {
                    Some(stats) => stats_ui(&mut self.metric, ui, stats),
                    None => {
                        ui.heading(&entry.day);
                        ui.label(format!(
                            "{}. This is missing data, not a day of zeros.",
                            missing_label(&entry.state)
                        ));
                        if let Some(error) = &entry.error {
                            ui.colored_label(BAD, error);
                        }
                        (entry.state == "no_stats" && ui.button("Open journal").clicked())
                            .then(|| Action::OpenJournal(entry.day.clone()))
                    }
                }
            }
        }
    }
}

/// The week's recap: a bar per day, then the day panels (breakdowns, apps, cost) over the sum.
fn week_ui(metric: &mut Metric, ui: &mut egui::Ui, recap: &WeekRecap) -> Option<Action> {
    ui.heading(format!("Week {} to {}", recap.first_day, recap.last_day));
    if recap.total.partial {
        ui.colored_label(WARN, "Partial: a day was rebuilt by force or a source degraded.");
    }
    if !recap.missing.is_empty() {
        let named: Vec<String> = recap
            .missing
            .iter()
            .map(|(day, state)| format!("{day} ({})", missing_label(state).to_lowercase()))
            .collect();
        ui.colored_label(
            WARN,
            format!("No numbers for {}. Missing data, not days of zeros.", named.join(", ")),
        );
    }
    ui.label(format!(
        "{} events · {} active over {} day(s)",
        recap.total.total_events,
        duration(recap.total.total_active_seconds),
        recap.per_day.len()
    ));
    let bars = recap
        .per_day
        .iter()
        .map(|(day, seconds)| {
            Bar::new(weekday_index(&recap.first_day, day), *seconds as f64 / 60.0)
                .width(0.7)
                .name(day)
        })
        .collect();
    Plot::new(("week", &recap.first_day))
        .height(180.0)
        .include_x(-0.5)
        .include_x(6.5)
        .include_y(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label("Mon … Sun")
        .y_axis_label("Active minutes")
        .show(ui, |plot| plot.bar_chart(BarChart::new("Active minutes", bars)));
    ui.separator();
    // The week's own line above says what is partial; the day panels' "rebuilt by force" would
    // be the wrong wording for a sum.
    let sums = DayStats {
        partial: false,
        ..recap.total.clone()
    };
    stats_panels(metric, ui, &sums);
    None
}

/// 0 for the week's Monday through 6 for its Sunday.
fn weekday_index(first: &str, day: &str) -> f64 {
    let parse = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    match (parse(first), parse(day)) {
        (Some(a), Some(b)) => (b - a).num_days() as f64,
        _ => 0.0,
    }
}

/// One day's panels, 2c.1 to 2c.4, from its stats document.
fn stats_ui(metric: &mut Metric, ui: &mut egui::Ui, stats: &DayStats) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(&stats.day);
        ui.label(RichText::new(&stats.timezone).weak());
        if ui.button("Open journal").clicked() {
            action = Some(Action::OpenJournal(stats.day.clone()));
        }
    });
    stats_panels(metric, ui, stats);
    action
}

/// Everything under a day's heading: counts, hourly chart, breakdowns, apps and cost.
fn stats_panels(metric: &mut Metric, ui: &mut egui::Ui, stats: &DayStats) {
    // 2c.1: counts and sources; a degraded source is named, not shown as zero.
    ui.label(format!(
        "{} events · {} active",
        stats.total_events,
        duration(stats.total_active_seconds)
    ));
    let degraded = stats.degraded();
    if !degraded.is_empty() {
        let named: Vec<String> = stats
            .sources
            .iter()
            .filter(|(n, _)| degraded.contains(&n.as_str()))
            .map(|(n, s)| format!("{n} ({s})"))
            .collect();
        ui.colored_label(
            WARN,
            format!("Degraded: {}. Its numbers are incomplete.", named.join(", ")),
        );
    } else if stats.partial {
        ui.colored_label(WARN, "Partial: the day was rebuilt by force.");
    }
    if !stats.sources.is_empty() {
        ui.label(
            RichText::new(
                stats
                    .sources
                    .iter()
                    .map(|(n, s)| format!("{n}: {s}"))
                    .collect::<Vec<_>>()
                    .join(" · "),
            )
            .weak(),
        );
    }
    if !reconciles(stats) {
        ui.colored_label(BAD, "The hourly rows do not add up to the engine's totals.");
    }
    ui.separator();

    // 2c.2: the hourly timeline, stacked by project.
    ui.horizontal(|ui| {
        ui.label(RichText::new("By hour").strong());
        ui.selectable_value(metric, Metric::Active, Metric::Active.label());
        ui.selectable_value(metric, Metric::Events, Metric::Events.label());
    });
    let mut charts: Vec<BarChart> = Vec::new();
    for (project, hours) in hourly(stats, *metric) {
        let bars = hours
            .iter()
            .enumerate()
            .map(|(hour, value)| Bar::new(hour as f64, *value).width(0.8))
            .collect();
        let mut chart = BarChart::new(project, bars);
        let below: Vec<&BarChart> = charts.iter().collect();
        if !below.is_empty() {
            chart = chart.stack_on(&below);
        }
        charts.push(chart);
    }
    Plot::new(("hours", &stats.day))
        .height(220.0)
        .legend(Legend::default())
        .include_x(-0.5)
        .include_x(23.5)
        .include_y(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .x_axis_label("Local hour")
        .y_axis_label(metric.label())
        .show(ui, |plot| {
            for chart in charts {
                plot.bar_chart(chart);
            }
        });
    ui.separator();

    // 2c.3: project and source breakdowns, each totalling the counts above.
    ui.columns(2, |columns| {
        share_table(&mut columns[0], "Projects", &by_project(stats));
        share_table(&mut columns[1], "Sources", &by_source(stats));
    });
    ui.separator();
    ui.columns(2, |columns| {
        columns[0].label(RichText::new("Top apps").strong());
        for (name, seconds) in &stats.top_apps {
            columns[0].label(format!("{name} — {}", duration(*seconds)));
        }
        columns[1].label(RichText::new("Top domains").strong());
        for (name, visits) in &stats.top_domains {
            columns[1].label(format!("{name} — {visits} visits"));
        }
    });
    ui.separator();

    // 2c.4: cost, from the same usage events as the journal's Cost section.
    ui.label(RichText::new("Cost").strong());
    if stats.cost.is_empty() {
        ui.label("No AI usage recorded.");
    } else {
        egui::Grid::new(("cost", &stats.day))
            .striped(true)
            .num_columns(4)
            .show(ui, |ui| {
                for header in ["Provider", "Requests", "Tokens", "USD"] {
                    ui.label(RichText::new(header).strong());
                }
                ui.end_row();
                for row in &stats.cost {
                    ui.label(&row.provider);
                    ui.label(row.requests.to_string());
                    ui.label(row.total_tokens.to_string());
                    ui.label(format!("{:.4}", row.cost_usd));
                    ui.end_row();
                }
                ui.label(RichText::new("Total").strong());
                ui.label("");
                ui.label("");
                ui.label(RichText::new(format!("{:.4}", cost_total(stats))).strong());
                ui.end_row();
            });
    }
}

fn share_table(ui: &mut egui::Ui, title: &str, shares: &[Share]) {
    ui.label(RichText::new(title).strong());
    egui::Grid::new(title).striped(true).num_columns(3).show(ui, |ui| {
        for share in shares {
            ui.label(&share.name);
            ui.label(duration(share.active_seconds));
            ui.label(format!("{} events", share.events));
            ui.end_row();
        }
    });
}

fn missing_label(state: &str) -> &'static str {
    match state {
        "no_stats" => "Published before stats existed",
        "no_journal" => "Nothing published",
        "unreadable" => "Stats file unreadable",
        _ => "No stats",
    }
}

fn failure(parse_error: String, result: &QueryResult) -> String {
    let tail: Vec<&str> = result.stderr.lines().rev().take(3).collect();
    let detail = tail.into_iter().rev().collect::<Vec<_>>().join(" ");
    match result.code {
        Some(code) if detail.is_empty() => format!("{parse_error} (exit {code})"),
        Some(code) => format!("{parse_error} (exit {code}): {detail}"),
        None => format!("{parse_error}: {detail}"),
    }
}

/// The day id `count` days before `day`, or `None` when `day` is not `YYYY-MM-DD`.
fn days_before(day: &str, count: u32) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(
        (date - chrono::Duration::days(i64::from(count)))
            .format("%Y-%m-%d")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_counts_back_across_months() {
        assert_eq!(days_before("2026-11-03", 6).as_deref(), Some("2026-10-28"));
        assert_eq!(days_before("not a day", 1), None);
    }
}
