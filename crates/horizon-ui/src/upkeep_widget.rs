//! The Upkeep board: what the hourly dependency agent did, does now and will do next.
//!
//! It only reads the status the agent writes. Opening a pull request is a request the
//! app carries out after drawing.
use std::time::Duration;

use egui::{Color32, CornerRadius, Frame, Id, Margin, RichText, ScrollArea, Stroke, Ui, Vec2};
use horizon_core::Panel;
use horizon_core::upkeep::time::{self, OffsetDateTime};
use horizon_core::upkeep::{Checks, PrState, Status};

use crate::theme;

mod header;
mod list;
mod runs;

/// Merged pull requests older than this leave the board.
const MERGED_WINDOW: time::Duration = time::Duration::days(7);
/// How far ahead of this computer's clock a time may be and still count.
const CLOCK_SKEW: time::Duration = time::Duration::minutes(5);
/// How long Run now stays unavailable after a press while the status does not show the run:
/// two exchanges and the loop's own wait. Measured on the monotonic clock.
const RUN_REQUEST_WAIT: Duration = Duration::from_secs(120);
/// How far the worker's clock may lag this computer's for a run to count as the one asked for.
const RUN_CLOCK_SKEW: time::Duration = time::Duration::seconds(30);

pub struct UpkeepView<'a> {
    panel: &'a mut Panel,
}

impl<'a> UpkeepView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Draws the board. Returns true when the pointer is over it, for focus tracking.
    pub fn show(&mut self, ui: &mut Ui, _is_focused: bool) -> bool {
        let hovered = ui.rect_contains_pointer(ui.max_rect());
        let panel_id = self.panel.id.0;
        let Some(board) = self.panel.content.upkeep_mut() else {
            return hovered;
        };
        board.poll();
        // The status file is looked at every two seconds; relative times change by the minute.
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let now = OffsetDateTime::now_utc();
        let filter_id = Id::new(("upkeep-filter", panel_id));
        let mut filter = ui.data(|data| data.get_temp::<Filter>(filter_id)).unwrap_or_default();

        ScrollArea::vertical()
            .id_salt(("upkeep", panel_id))
            .auto_shrink(false)
            .show(ui, |ui| {
                Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    let Some(status) = board.status.clone() else {
                        empty(ui, board.error.as_deref(), &board.path().display().to_string());
                        return;
                    };
                    // Run now waits for the status to show its run, at most a few minutes.
                    let running = runs::live(&status, now).is_some();
                    if running
                        || board
                            .run_requested
                            .is_some_and(|(at, pressed)| run_request_done(&status, at, pressed.elapsed(), now))
                    {
                        board.run_requested = None;
                    }
                    let actions = board.cloud.is_some().then_some(header::Actions {
                        busy: running || board.run_requested.is_some(),
                    });
                    if let Some(request) = header::show(ui, &status, actions) {
                        if request == horizon_core::upkeep::Request::RunNow {
                            board.run_requested = Some((now, std::time::Instant::now()));
                        }
                        board.request = Some(request);
                    }
                    if let Some(error) = board.error.as_deref() {
                        // The last status stays, marked as old, until a valid one arrives.
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{error} The board shows the last status it read."))
                                    .size(12.0)
                                    .color(theme::PALETTE_YELLOW()),
                            )
                            .wrap(),
                        );
                    }
                    ui.add_space(4.0);
                    runs::show(ui, &status, now);
                    ui.add_space(10.0);
                    let shown = shown_pull_requests(&status, now);
                    list::chips(ui, &shown, &mut filter);
                    ui.add_space(6.0);
                    if let Some(url) = list::show(ui, &shown, filter, now) {
                        board.request = Some(horizon_core::upkeep::Request::Open(url));
                    }
                    ui.add_space(12.0);
                    activity(ui, &status);
                });
            });
        ui.data_mut(|data| data.insert_temp(filter_id, filter));
        hovered
    }
}

/// Whether Run now pressed at `at`, `waited` ago, no longer waits: the status shows a run
/// that started after the press, also one that ended at once, or the wait is over. A start
/// in the future is not believed.
fn run_request_done(status: &Status, at: OffsetDateTime, waited: Duration, now: OffsetDateTime) -> bool {
    waited > RUN_REQUEST_WAIT
        || status
            .runs
            .iter()
            .any(|run| run.started_at >= at - RUN_CLOCK_SKEW && run.started_at <= now + CLOCK_SKEW)
}

/// Which pull requests the list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    Open,
    NeedsPerson,
    State(PrState),
}

impl Filter {
    fn admits(self, state: PrState) -> bool {
        match self {
            Self::Open => state != PrState::Merged,
            Self::NeedsPerson => state.needs_person(),
            Self::State(wanted) => state == wanted,
        }
    }
}

/// Open pull requests and those merged within the window, newest change first.
fn shown_pull_requests(status: &Status, now: OffsetDateTime) -> Vec<&horizon_core::upkeep::PullRequest> {
    let mut shown: Vec<_> = status
        .pull_requests
        .iter()
        .filter(|pr| {
            pr.state != PrState::Merged
                || pr
                    .updated_at
                    .is_some_and(|merged| merged <= now + CLOCK_SKEW && now - merged <= MERGED_WINDOW)
        })
        .collect();
    shown.sort_by_key(|pr| std::cmp::Reverse(pr.updated_at));
    shown
}

pub(crate) fn state_color(state: PrState) -> Color32 {
    match state {
        PrState::Blocked => theme::PALETTE_RED(),
        PrState::NeedsReview => theme::PALETTE_YELLOW(),
        PrState::Fixing => theme::ACCENT(),
        PrState::WaitingChecks => theme::PALETTE_CYAN(),
        PrState::Ready => theme::PALETTE_GREEN(),
        PrState::Merged => theme::FG_SOFT(),
        PrState::Other => theme::FG_DIM(),
    }
}

pub(crate) fn checks_look(checks: Checks) -> (Color32, &'static str) {
    match checks {
        Checks::Passed => (theme::PALETTE_GREEN(), "Checks passed"),
        Checks::Failed => (theme::PALETTE_RED(), "Checks failed"),
        Checks::Running => (theme::PALETTE_YELLOW(), "Checks running"),
        Checks::None => (theme::FG_DIM(), "No checks"),
    }
}

/// A read-only tag in a state color: the pill recipe of the design guidelines.
pub(crate) fn pill(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(theme::alpha(color, 34))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).size(11.0).strong().color(color)).selectable(false));
        });
}

pub(crate) fn caption(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text.to_uppercase())
                .size(10.5)
                .strong()
                .color(theme::FG_DIM()),
        )
        .selectable(false),
    );
}

fn empty(ui: &mut Ui, error: Option<&str>, path: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Upkeep").size(18.0).strong().color(theme::FG()));
        ui.add_space(4.0);
        let line = error.unwrap_or("Upkeep has not reported yet.");
        ui.label(RichText::new(line).size(13.0).color(theme::FG_SOFT()));
        ui.label(
            RichText::new(format!("The board reads {path}"))
                .size(11.5)
                .monospace()
                .color(theme::FG_DIM()),
        );
    });
}

/// The latest run's own account of what it did.
fn activity(ui: &mut Ui, status: &Status) {
    if status.activity.is_empty() {
        return;
    }
    caption(ui, "Latest run");
    Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 5.0;
            for entry in status.activity.iter().rev().take(8).rev() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [76.0, 16.0],
                        egui::Label::new(
                            RichText::new(horizon_core::upkeep::clock(entry.at))
                                .size(11.0)
                                .monospace()
                                .color(theme::FG_DIM()),
                        )
                        .selectable(false),
                    );
                    ui.add(egui::Label::new(RichText::new(&entry.text).size(12.0).color(theme::FG_SOFT())).truncate());
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::upkeep::parse_time;

    fn status(pull_requests: &str) -> Status {
        serde_json::from_str(&format!(r#"{{"pull_requests":[{pull_requests}]}}"#)).unwrap()
    }

    #[test]
    fn the_list_shows_open_and_recently_merged_pull_requests_newest_first() {
        let status = status(
            r#"{"repository":"acme/web","number":1,"title":"a","url":"","state":"merged","updated_at":"2026-10-01T10:00:00Z"},
               {"repository":"acme/web","number":2,"title":"b","url":"","state":"merged","updated_at":"2026-10-12T10:00:00Z"},
               {"repository":"acme/web","number":3,"title":"c","url":"","state":"blocked","updated_at":"2026-10-13T09:00:00Z"},
               {"repository":"acme/web","number":4,"title":"d","url":"","state":"merged"},
               {"repository":"acme/web","number":6,"title":"f","url":"","state":"merged","updated_at":"2099-01-01T00:00:00Z"},
               {"repository":"acme/api","number":5,"title":"e","url":"","state":"ready"}"#,
        );
        let now = parse_time("2026-10-13T12:00:00Z").unwrap();
        let shown: Vec<u64> = shown_pull_requests(&status, now).iter().map(|pr| pr.number).collect();
        assert_eq!(
            shown,
            [3, 2, 5],
            "older than a week or with no merge time leaves the board"
        );
    }

    #[test]
    fn run_now_waits_for_a_run_after_the_press_and_not_longer_than_its_limit() {
        let at = parse_time("2026-10-13T14:00:00Z").unwrap();
        let runs = |started: &str| -> Status {
            serde_json::from_str(&format!(
                r#"{{"runs":[{{"started_at":"{started}","outcome":"done"}}]}}"#
            ))
            .unwrap()
        };
        let short = Duration::from_secs(10);
        assert!(run_request_done(&runs("2026-10-13T14:00:20Z"), at, short, at));
        assert!(
            !run_request_done(&runs("2026-10-13T13:00:00Z"), at, short, at),
            "an older run"
        );
        assert!(
            !run_request_done(&runs("2027-01-01T00:00:00Z"), at, short, at),
            "a run dated far ahead is not the one asked for"
        );
        assert!(run_request_done(
            &runs("2026-10-13T13:00:00Z"),
            at,
            Duration::from_secs(121),
            at
        ));
    }

    #[test]
    fn filters_pick_what_a_person_must_look_at() {
        assert!(Filter::Open.admits(PrState::Ready) && !Filter::Open.admits(PrState::Merged));
        assert!(Filter::NeedsPerson.admits(PrState::Blocked) && Filter::NeedsPerson.admits(PrState::NeedsReview));
        assert!(!Filter::NeedsPerson.admits(PrState::Fixing));
        assert!(Filter::State(PrState::Merged).admits(PrState::Merged));
    }
}
