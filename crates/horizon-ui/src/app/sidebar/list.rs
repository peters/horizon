//! The groups of the sidebar's cloud list and the rows it keeps. A row reads its
//! status line from a terminal's screen, so the rows are read again at most once a
//! second, or at once when workspaces come or go.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use horizon_core::cloud_list::{self, Group, Row};
use horizon_core::{AgentStatus, WorkspaceId};

use crate::theme;

use super::super::HorizonApp;
use super::text;
use super::{SidebarActions, SidebarWorkspaceDragState, WorkspaceSidebarEntry};

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
    pub(in crate::app) fn refresh_sidebar_rows(&mut self, now: Instant) {
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

    /// The groups with their headers and rows. A group without rows is not shown.
    pub(super) fn render_sidebar_groups(
        &mut self,
        ui: &mut egui::Ui,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        for group in Group::ALL {
            let rows = workspace_data.iter().filter(|entry| entry.row.group == group);
            let count = rows.clone().count();
            if count == 0 {
                continue;
            }
            let summary = cloud_list::group_summary(group, rows.clone().map(|entry| &entry.row));
            render_group_header(ui, group, count, &summary);
            for workspace in rows {
                self.render_sidebar_workspace(ui, workspace, actions, drag_state);
            }
        }
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

const HEADER_HEIGHT: f32 = 34.0;
const HEADER_TEXT_SIZE: f32 = 12.0;
const SUMMARY_TEXT_SIZE: f32 = 10.5;
const HEADER_LEFT_MARGIN: f32 = 18.0;
const HEADER_RIGHT_MARGIN: f32 = 14.0;
/// The least room for the summary: about one short part, such as `live`.
const MIN_SUMMARY_WIDTH: f32 = 24.0;

/// The header of a group: its name, how many workspaces it has and, at the right,
/// its summary on the same line.
pub(super) fn render_group_header(ui: &mut egui::Ui, group: Group, count: usize, summary: &[String]) {
    let rect = ui.allocate_space(egui::vec2(ui.available_width(), HEADER_HEIGHT)).1;
    let center_y = rect.max.y - 13.0;
    let (caption, x) = paint_caption(ui, rect, group, count, center_y);

    let right = rect.max.x - HEADER_RIGHT_MARGIN;
    // A narrow header leaves the summary out rather than draw it over the caption;
    // a screen reader then hears it with the group name.
    let summary_fits = right - x >= MIN_SUMMARY_WIDTH;
    let caption_response = ui.interact(
        caption,
        ui.make_persistent_id(("sidebar_group_caption", group.key())),
        egui::Sense::hover(),
    );
    caption_response.widget_info(|| {
        let mut spoken = format!("{} {count}", group.label());
        if !summary_fits && !summary.is_empty() {
            spoken = format!("{spoken}, {}", summary.join(" · "));
        }
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken)
    });
    if !summary_fits {
        return;
    }
    let Some(fitted) = fitted_summary(summary, right - x, |text| {
        ui.painter()
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(SUMMARY_TEXT_SIZE),
                theme::FG_DIM(),
            )
            .size()
            .x
    }) else {
        return;
    };
    let galley = text::line(
        ui,
        &fitted,
        SUMMARY_TEXT_SIZE,
        text::REGULAR,
        theme::FG_DIM(),
        (right - x).max(0.0),
    );
    let summary_rect = egui::Rect::from_min_size(
        egui::pos2(right - galley.size().x, center_y - galley.size().y / 2.0),
        galley.size(),
    );
    ui.painter().galley(summary_rect.min, galley, theme::FG_DIM());
    // A screen reader gets the whole summary, also when the header drops parts.
    let full = summary.join(" · ");
    let response = ui.interact(
        summary_rect,
        ui.make_persistent_id(("sidebar_group_summary", group.key())),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &full));
    response.on_hover_text(full);
}

/// Paints the group name and its count at the left of the header. Returns the
/// rect of both and where the next part starts.
fn paint_caption(ui: &egui::Ui, rect: egui::Rect, group: Group, count: usize, center_y: f32) -> (egui::Rect, f32) {
    let painter = ui.painter();
    let left = rect.min.x + HEADER_LEFT_MARGIN;
    let label = text::line(
        ui,
        group.label(),
        HEADER_TEXT_SIZE,
        text::SEMIBOLD,
        theme::FG_SOFT(),
        rect.width(),
    );
    let label_width = label.size().x;
    painter.galley(
        egui::pos2(left, center_y - label.size().y / 2.0),
        label,
        theme::FG_SOFT(),
    );
    let count_left = left + label_width + 7.0;
    let count = text::mono(ui, &count.to_string(), 11.0, theme::FG_DIM());
    let count_right = count_left + count.size().x;
    painter.galley(
        egui::pos2(count_left, center_y - count.size().y / 2.0),
        count,
        theme::FG_DIM(),
    );
    (
        egui::Rect::from_x_y_ranges(left..=count_right, rect.y_range()),
        count_right + 8.0,
    )
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
