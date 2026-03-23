use std::sync::mpsc;
use std::time::Instant;

use egui::{Align2, Context, CornerRadius, Rect, Sense, Vec2};
use horizon_core::SshConnection;

use crate::dir_picker::{PickerEmptyState, PickerModalAction, PickerModalConfig, PickerModalState};
use crate::theme;

use super::join_remote_browser_path;
use super::render::paint_folder_icon;
use super::worker::{RemoteDirectoryListing, spawn_remote_directory_listing};

const REMOTE_DEBOUNCE_MS: u64 = 120;
const ROW_HEIGHT: f32 = 34.0;

pub(super) struct RemoteDestinationPicker {
    modal: PickerModalState,
    results: Vec<RemoteDestinationEntry>,
    current_dir: String,
    error: Option<String>,
    pending_listing: Option<PendingRemoteListing>,
    last_query_sent: String,
    last_query_time: Instant,
    initial_results_loaded: bool,
}

pub(super) enum RemoteDestinationPickerAction {
    None,
    Cancelled,
    Selected(String),
}

struct RemoteDestinationEntry {
    name: String,
    resolved_path: String,
}

struct PendingRemoteListing {
    requested_list_path: String,
    rx: mpsc::Receiver<Result<RemoteDirectoryListing, String>>,
}

struct RemoteQueryRequest {
    list_path: String,
    prefix: String,
}

impl RemoteDestinationPicker {
    pub(super) fn new(initial_query: &str, connection: &SshConnection) -> Self {
        let query = initial_query.trim().to_string();
        let request = remote_query_request(&query);
        Self {
            modal: PickerModalState::new(query.clone()),
            results: Vec::new(),
            current_dir: String::new(),
            error: None,
            pending_listing: Some(PendingRemoteListing::spawn(connection, request.list_path)),
            last_query_sent: query,
            last_query_time: Instant::now(),
            initial_results_loaded: false,
        }
    }

    pub(super) fn show(&mut self, ctx: &Context, connection: &SshConnection) -> RemoteDestinationPickerAction {
        self.update_search(connection);

        let status_line = (!self.current_dir.is_empty()).then(|| format!("Browsing: {}", self.current_dir));
        let empty_state = if let Some(error) = &self.error {
            PickerEmptyState {
                message: error,
                color: theme::PALETTE_RED,
            }
        } else if self.initial_results_loaded {
            PickerEmptyState {
                message: "No remote directories found",
                color: theme::FG_DIM,
            }
        } else {
            PickerEmptyState {
                message: "Loading remote directories...",
                color: theme::FG_DIM,
            }
        };

        let action = self.modal.show(
            ctx,
            &PickerModalConfig {
                id_source: "ssh_upload_destination_picker",
                heading: "Select remote destination",
                hint_text: "Type a remote path or browse...",
                status_text: status_line.as_deref(),
                empty_state,
                footer_action_label: Some("Use typed path"),
            },
            &self.results,
            render_remote_result_row,
        );

        match action {
            PickerModalAction::None => RemoteDestinationPickerAction::None,
            PickerModalAction::Cancelled => RemoteDestinationPickerAction::Cancelled,
            PickerModalAction::Submit | PickerModalAction::FooterAction => self.confirm_selection(),
            PickerModalAction::CompleteSelection => {
                self.complete_selection();
                RemoteDestinationPickerAction::None
            }
            PickerModalAction::ClickedRow(index) => {
                RemoteDestinationPickerAction::Selected(self.results[index].resolved_path.clone())
            }
        }
    }

    fn update_search(&mut self, connection: &SshConnection) {
        if let Some(pending) = self.pending_listing.take() {
            match pending.rx.try_recv() {
                Ok(Ok(listing)) => {
                    if pending.matches_query(self.modal.query()) {
                        self.apply_listing(listing);
                        self.error = None;
                        self.initial_results_loaded = true;
                    }
                }
                Ok(Err(error)) => {
                    if pending.matches_query(self.modal.query()) {
                        self.results.clear();
                        self.error = Some(error);
                        self.initial_results_loaded = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.pending_listing = Some(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if pending.matches_query(self.modal.query()) {
                        self.results.clear();
                        self.error = Some("remote directory picker disconnected".to_string());
                        self.initial_results_loaded = true;
                    }
                }
            }
        }

        if self.modal.query() != self.last_query_sent
            && self.last_query_time.elapsed().as_millis() >= u128::from(REMOTE_DEBOUNCE_MS)
        {
            let request = remote_query_request(self.modal.query());
            self.last_query_sent.clear();
            self.last_query_sent.push_str(self.modal.query());
            self.last_query_time = Instant::now();
            self.error = None;
            self.pending_listing = Some(PendingRemoteListing::spawn(connection, request.list_path));
        }
    }

    fn apply_listing(&mut self, listing: RemoteDirectoryListing) {
        let request = remote_query_request(self.modal.query());
        self.current_dir.clone_from(&listing.current_dir);
        self.results = listing
            .entries
            .into_iter()
            .filter(|entry| remote_entry_matches(entry, &request.prefix))
            .map(|name| RemoteDestinationEntry {
                resolved_path: join_remote_browser_path(&listing.current_dir, &name),
                name,
            })
            .collect();
        self.modal.clamp_selected(self.results.len());
    }

    fn complete_selection(&mut self) {
        let Some(entry) = self.results.get(self.modal.selected_index()) else {
            return;
        };

        self.modal.set_query(browse_query_for(&entry.resolved_path));
        self.last_query_time = Instant::now();
    }

    fn confirm_selection(&mut self) -> RemoteDestinationPickerAction {
        let query = self.modal.query().trim();
        if query_targets_current_dir(query, &self.current_dir) {
            return RemoteDestinationPickerAction::Selected(self.current_dir.clone());
        }

        if let Some(entry) = self.results.get(self.modal.selected_index()) {
            return RemoteDestinationPickerAction::Selected(entry.resolved_path.clone());
        }

        if !query.is_empty() {
            return RemoteDestinationPickerAction::Selected(query.to_string());
        }

        if !self.current_dir.is_empty() {
            return RemoteDestinationPickerAction::Selected(self.current_dir.clone());
        }

        RemoteDestinationPickerAction::None
    }
}

impl PendingRemoteListing {
    fn spawn(connection: &SshConnection, requested_list_path: String) -> Self {
        Self {
            rx: spawn_remote_directory_listing(connection.clone(), requested_list_path.clone()),
            requested_list_path,
        }
    }

    fn matches_query(&self, query: &str) -> bool {
        self.requested_list_path == remote_query_request(query).list_path
    }
}

fn remote_query_request(query: &str) -> RemoteQueryRequest {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return RemoteQueryRequest {
            list_path: String::new(),
            prefix: String::new(),
        };
    }

    if trimmed == "/" {
        return RemoteQueryRequest {
            list_path: "/".to_string(),
            prefix: String::new(),
        };
    }

    if let Some(path) = trimmed.strip_suffix('/') {
        let list_path = if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        };
        return RemoteQueryRequest {
            list_path,
            prefix: String::new(),
        };
    }

    if let Some((parent, prefix)) = trimmed.rsplit_once('/') {
        let list_path = if parent.is_empty() {
            "/".to_string()
        } else {
            parent.to_string()
        };
        return RemoteQueryRequest {
            list_path,
            prefix: prefix.to_string(),
        };
    }

    RemoteQueryRequest {
        list_path: String::new(),
        prefix: trimmed.to_string(),
    }
}

fn browse_query_for(path: &str) -> String {
    if path == "/" {
        "/".to_string()
    } else {
        format!("{path}/")
    }
}

fn remote_entry_matches(entry: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }

    entry.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase())
}

fn query_targets_current_dir(query: &str, current_dir: &str) -> bool {
    !current_dir.is_empty() && (query == current_dir || query == browse_query_for(current_dir))
}

fn render_remote_result_row(
    ui: &mut egui::Ui,
    width: f32,
    index: usize,
    entry: &RemoteDestinationEntry,
    is_selected: bool,
) -> bool {
    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(
                row_rect,
                ui.make_persistent_id(("remote_dir_hover", index)),
                Sense::hover(),
            )
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    let click = ui.interact(
        row_rect,
        ui.make_persistent_id(("remote_dir_click", index)),
        Sense::click(),
    );
    if click.clicked() {
        clicked = true;
    }

    let icon_rect = Rect::from_min_size(
        egui::pos2(row_rect.min.x + 8.0, row_rect.center().y - 8.0),
        Vec2::new(16.0, 16.0),
    );
    paint_folder_icon(&ui.painter_at(row_rect), icon_rect);

    let text_color = if entry.name == ".." {
        theme::FG_DIM
    } else {
        theme::FG_SOFT
    };
    ui.painter_at(row_rect).text(
        egui::pos2(row_rect.min.x + 30.0, row_rect.center().y),
        Align2::LEFT_CENTER,
        &entry.name,
        egui::FontId::proportional(12.0),
        if is_selected { theme::FG } else { text_color },
    );

    clicked
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use horizon_core::SshConnection;

    use super::{
        PendingRemoteListing, RemoteDestinationEntry, RemoteDestinationPicker, RemoteDirectoryListing,
        browse_query_for, query_targets_current_dir, remote_entry_matches, remote_query_request,
    };

    #[test]
    fn remote_query_request_uses_parent_for_partial_leaf() {
        let request = remote_query_request("/srv/log");
        assert_eq!(request.list_path, "/srv");
        assert_eq!(request.prefix, "log");
    }

    #[test]
    fn remote_query_request_keeps_directory_queries() {
        let request = remote_query_request("~/projects/");
        assert_eq!(request.list_path, "~/projects");
        assert!(request.prefix.is_empty());
    }

    #[test]
    fn remote_entry_matches_is_case_insensitive_prefix_match() {
        assert!(remote_entry_matches("ComputeCache", "comp"));
        assert!(!remote_entry_matches("ComputeCache", "cache"));
    }

    #[test]
    fn browse_query_for_preserves_root() {
        assert_eq!(browse_query_for("/"), "/");
        assert_eq!(browse_query_for("/srv/logs"), "/srv/logs/");
    }

    #[test]
    fn query_targets_current_dir_accepts_trailing_slash() {
        assert!(query_targets_current_dir("/srv/logs/", "/srv/logs"));
        assert!(query_targets_current_dir("/srv/logs", "/srv/logs"));
        assert!(!query_targets_current_dir("/srv", "/srv/logs"));
    }

    #[test]
    fn pending_listing_matches_prefix_changes_within_same_directory() {
        let pending = PendingRemoteListing {
            requested_list_path: "/srv".to_string(),
            rx: mpsc::channel().1,
        };

        assert!(pending.matches_query("/srv/log"));
    }

    #[test]
    fn pending_listing_rejects_stale_directory_changes() {
        let pending = PendingRemoteListing {
            requested_list_path: "/srv".to_string(),
            rx: mpsc::channel().1,
        };

        assert!(!pending.matches_query("/tmp/log"));
    }

    #[test]
    fn update_search_discards_stale_remote_listing_results() {
        let (tx, rx) = mpsc::channel();
        let mut picker = RemoteDestinationPicker {
            modal: super::PickerModalState::new("/tmp/log"),
            results: vec![super::RemoteDestinationEntry {
                name: "kept".to_string(),
                resolved_path: "/tmp/kept".to_string(),
            }],
            current_dir: "/tmp".to_string(),
            error: None,
            pending_listing: Some(PendingRemoteListing {
                requested_list_path: "/srv".to_string(),
                rx,
            }),
            last_query_sent: "/srv/log".to_string(),
            last_query_time: Instant::now(),
            initial_results_loaded: false,
        };

        tx.send(Ok(RemoteDirectoryListing {
            current_dir: "/srv".to_string(),
            entries: vec!["logs".to_string()],
        }))
        .expect("test listing should send");

        picker.update_search(&SshConnection::default());

        assert_eq!(picker.current_dir, "/tmp");
        assert_eq!(picker.results.len(), 1);
        assert_eq!(picker.results[0].name, "kept");
        assert!(!picker.initial_results_loaded);
    }

    #[test]
    fn confirm_selection_prefers_current_directory_after_completion() {
        let mut picker = RemoteDestinationPicker {
            modal: super::PickerModalState::new("/srv/logs/nested/"),
            results: vec![RemoteDestinationEntry {
                name: "..".to_string(),
                resolved_path: "/srv/logs".to_string(),
            }],
            current_dir: "/srv/logs/nested".to_string(),
            error: None,
            pending_listing: None,
            last_query_sent: "/srv/logs/nested/".to_string(),
            last_query_time: Instant::now(),
            initial_results_loaded: true,
        };

        assert!(matches!(
            picker.confirm_selection(),
            super::RemoteDestinationPickerAction::Selected(path) if path == "/srv/logs/nested"
        ));
    }
}
