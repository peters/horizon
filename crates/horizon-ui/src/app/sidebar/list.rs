//! The groups of the sidebar's cloud list and the rows it keeps. A row reads its
//! status line from a terminal's screen, so the rows are read again at most once a
//! second, or at once when workspaces come or go.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use horizon_core::cloud_list::{self, Group, Row};
use horizon_core::{AgentStatus, WorkspaceId};

use crate::theme;

use super::super::HorizonApp;
use super::WorkspaceSidebarEntry;

const REFRESH: Duration = Duration::from_secs(1);

/// The rows of the last read, by workspace.
#[derive(Default)]
pub(in crate::app) struct ListCache {
    refreshed: Option<Instant>,
    /// The id and persistent local id of each workspace: ids restart at 1 in a
    /// replaced session, so the id alone does not tell the workspaces apart.
    workspaces: Vec<(WorkspaceId, String)>,
    rows: HashMap<WorkspaceId, Row>,
}

impl HorizonApp {
    /// Reads the rows again when they are a second old or the workspaces changed.
    pub(super) fn refresh_sidebar_rows(&mut self, now: Instant) {
        let cache = &self.sidebar_list;
        let unchanged = cache.workspaces.iter().map(|(id, local)| (*id, local.as_str())).eq(self
            .board
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.local_id.as_str())));
        if unchanged && cache.refreshed.is_some_and(|at| now.duration_since(at) < REFRESH) {
            return;
        }
        self.sidebar_list = ListCache {
            refreshed: Some(now),
            workspaces: self
                .board
                .workspaces
                .iter()
                .map(|workspace| (workspace.id, workspace.local_id.clone()))
                .collect(),
            rows: self.read_sidebar_rows(),
        };
    }

    /// The row of each workspace now.
    pub(in crate::app) fn read_sidebar_rows(&self) -> HashMap<WorkspaceId, Row> {
        #[cfg(feature = "cloud-workspaces")]
        let mut clouds = self.cloud_list_facts(std::time::SystemTime::now());
        #[cfg(not(feature = "cloud-workspaces"))]
        let mut clouds: HashMap<String, Vec<cloud_list::CloudFacts>> = HashMap::new();
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let facts = clouds.remove(&workspace.local_id).unwrap_or_default();
                let panels = workspace.panels.iter().filter_map(|id| self.board.panel(*id));
                let row = if facts.is_empty() {
                    let working = panels.clone().any(|panel| panel.agent_status() == AgentStatus::Working);
                    let line = cloud_list::primary_panel(panels, self.board.focused).and_then(cloud_list::panel_line);
                    Row::of(&[], working, line.as_deref())
                } else {
                    Row::of(&facts, false, None)
                };
                (workspace.id, row)
            })
            .collect()
    }

    /// The row of `workspace` from the last read; a workspace read for the first
    /// time shows on This PC until then.
    pub(super) fn sidebar_row(&self, workspace: WorkspaceId) -> Row {
        self.sidebar_list
            .rows
            .get(&workspace)
            .cloned()
            .unwrap_or_else(|| Row::of(&[], false, None))
    }
}

/// Whether a workspace of the group `dragged` drops on the row `target`: only on a
/// row of its own group. The groups keep their order, so a row dropped into another
/// group would not show where it was dropped.
pub(super) fn accepts_drop(dragged: Group, target: &WorkspaceSidebarEntry) -> bool {
    dragged == target.row.group && super::sidebar_workspace_drop_should_dock(target.detached)
}

const HEADER_TEXT_SIZE: f32 = 10.5;
const HEADER_RIGHT_MARGIN: f32 = 14.0;

/// The header of a group: its name, how many workspaces it has and, at the right,
/// its summary on the same line.
pub(super) fn render_group_header(ui: &mut egui::Ui, group: Group, count: usize, summary: &[String]) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.label(
            egui::RichText::new(group.label().to_uppercase())
                .color(theme::FG_DIM())
                .size(HEADER_TEXT_SIZE)
                .strong(),
        );
        ui.label(
            egui::RichText::new(count.to_string())
                .color(theme::FG_DIM())
                .size(HEADER_TEXT_SIZE),
        );
        let width = ui.available_width() - HEADER_RIGHT_MARGIN - ui.spacing().item_spacing.x;
        let Some(text) = fitted_summary(summary, width, |text| {
            ui.painter()
                .layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(HEADER_TEXT_SIZE),
                    theme::FG_DIM(),
                )
                .size()
                .x
        }) else {
            return;
        };
        let full = summary.join(" · ");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(HEADER_RIGHT_MARGIN);
            let response = ui.add(
                egui::Label::new(egui::RichText::new(&text).color(theme::FG_DIM()).size(HEADER_TEXT_SIZE)).truncate(),
            );
            // A screen reader gets the whole summary, also when the header drops parts.
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &full));
            response.on_hover_text(full);
        });
    });
    ui.add_space(2.0);
}

/// The most parts of `summary` that fit in `width`, joined. When not even the first
/// part fits, the first part, which the label then truncates.
fn fitted_summary(summary: &[String], width: f32, measure: impl Fn(&str) -> f32) -> Option<String> {
    let first = summary.first()?;
    (2..=summary.len())
        .rev()
        .map(|parts| summary[..parts].join(" · "))
        .find(|text| measure(text) <= width)
        .or_else(|| Some(first.clone()))
}

#[cfg(test)]
mod tests {
    use super::fitted_summary;

    #[test]
    fn a_narrow_header_drops_the_last_parts_of_its_summary() {
        let summary = ["$0.254/h".to_owned(), "no local cost".to_owned()];
        let measure = |text: &str| f32::from(u8::try_from(text.chars().count()).unwrap_or(u8::MAX));
        assert_eq!(
            fitted_summary(&summary, 30.0, measure).as_deref(),
            Some("$0.254/h · no local cost")
        );
        assert_eq!(fitted_summary(&summary, 10.0, measure).as_deref(), Some("$0.254/h"));
        assert_eq!(fitted_summary(&summary, 2.0, measure).as_deref(), Some("$0.254/h"));
        assert_eq!(fitted_summary(&[], 100.0, measure), None);
    }

    #[test]
    fn a_replaced_session_with_the_same_workspace_ids_reads_the_rows_again() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let _workspace = app.board.create_workspace("a");
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(start),
            "same workspaces: the cache stays"
        );

        // The workspaces of a replaced session get the same ids from 1.
        for workspace in &mut app.board.workspaces {
            workspace.local_id = format!("{}-other-session", workspace.local_id);
        }
        app.refresh_sidebar_rows(soon);
        assert_eq!(app.sidebar_list.refreshed, Some(soon));
    }

    #[test]
    fn a_narrow_header_tells_a_screen_reader_its_whole_summary() {
        let summary = ["$0.254/h".to_owned(), "no local cost".to_owned()];
        let labels = crate::test_egui::accesskit_texts(|ui| {
            ui.allocate_ui(egui::vec2(150.0, 20.0), |ui| {
                super::render_group_header(ui, horizon_core::cloud_list::Group::Parked, 2, &summary);
            });
        });
        assert!(
            labels.iter().any(|(label, _)| label == "$0.254/h · no local cost"),
            "{labels:?}"
        );
    }
}
