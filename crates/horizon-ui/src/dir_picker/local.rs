use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use egui::{Context, CornerRadius, Pos2, Sense, Vec2};
use horizon_core::dir_search;
use horizon_core::{PresetConfig, WorkspaceId};

use super::{PickerEmptyState, PickerModalAction, PickerModalConfig, PickerModalState, split_path_display};
use crate::theme;

const DEBOUNCE_MS: u64 = 60;
const ROW_HEIGHT: f32 = 34.0;
/// The hint of the foot that says how a hidden folder reaches the list.
const TYPED_DOT_HINT: &str = "Type . to see hidden folders";

pub enum DirPickerPurpose {
    NewWorkspace {
        canvas_pos: [f32; 2],
        preset: PresetConfig,
    },
    AddPanel {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
}

pub struct DirPicker {
    modal: PickerModalState,
    results: Vec<dir_search::DirRow>,
    selected_purpose: Option<DirPickerPurpose>,
    search_rx: Option<mpsc::Receiver<Vec<dir_search::DirRow>>>,
    last_query_sent: String,
    last_query_time: Instant,
    initial_results_loaded: bool,
}

pub enum DirPickerAction {
    None,
    Selected(Option<PathBuf>, Box<DirPickerPurpose>),
    Cancelled,
}

impl DirPicker {
    pub fn new(purpose: DirPickerPurpose) -> Self {
        Self::with_seed(purpose, None)
    }

    pub fn with_seed(purpose: DirPickerPurpose, path: Option<&Path>) -> Self {
        let query = seed_query(path);
        let rx = dir_search::spawn_lookup(query.clone());
        Self {
            modal: PickerModalState::new(query.clone()),
            results: Vec::new(),
            selected_purpose: Some(purpose),
            search_rx: Some(rx),
            last_query_sent: query,
            last_query_time: Instant::now(),
            initial_results_loaded: false,
        }
    }

    pub fn show(&mut self, ctx: &Context) -> DirPickerAction {
        self.update_search();
        let empty_state = if self.initial_results_loaded {
            PickerEmptyState {
                message: "No directories found",
                color: theme::FG_DIM(),
            }
        } else {
            PickerEmptyState {
                message: "Searching...",
                color: theme::FG_DIM(),
            }
        };
        let action = self.modal.show(
            ctx,
            &PickerModalConfig {
                id_source: "dir_picker",
                heading: self.heading(),
                hint_text: "Type a path or search...",
                status_text: None,
                empty_state,
                footer_action_label: Some(Self::footer_action_label()),
                footer_hint: Some(TYPED_DOT_HINT),
            },
            &self.results,
            render_result_row,
        );

        match action {
            PickerModalAction::None => DirPickerAction::None,
            PickerModalAction::Cancelled => DirPickerAction::Cancelled,
            PickerModalAction::Submit => self.confirm_selection(),
            PickerModalAction::CompleteSelection => {
                self.complete_selection();
                DirPickerAction::None
            }
            PickerModalAction::ClickedRow(index) => self.select(Some(self.results[index].path.clone())),
            PickerModalAction::FooterAction => self.select(None),
        }
    }

    fn heading(&self) -> &'static str {
        match &self.selected_purpose {
            Some(DirPickerPurpose::NewWorkspace { .. }) => "Select workspace directory",
            Some(DirPickerPurpose::AddPanel { preset, .. }) if preset.kind.is_agent() => "Where should the agent work?",
            Some(DirPickerPurpose::AddPanel { .. }) | None => "Select terminal directory",
        }
    }

    const fn footer_action_label() -> &'static str {
        "Skip (use default)"
    }

    fn update_search(&mut self) {
        if let Some(rx) = &self.search_rx {
            match rx.try_recv() {
                Ok(results) => {
                    self.results = results;
                    self.modal.clamp_selected(self.results.len());
                    self.search_rx = None;
                    self.initial_results_loaded = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.search_rx = None;
                }
            }
        }

        if self.modal.query() != self.last_query_sent
            && self.last_query_time.elapsed().as_millis() >= u128::from(DEBOUNCE_MS)
        {
            self.last_query_sent.clear();
            self.last_query_sent.push_str(self.modal.query());
            self.last_query_time = Instant::now();
            self.search_rx = Some(dir_search::spawn_lookup(self.modal.query().to_string()));
        }
    }

    fn complete_selection(&mut self) {
        let Some(path) = self.results.get(self.modal.selected_index()) else {
            return;
        };

        self.modal
            .set_query(format!("{}/", dir_search::abbreviate_home(&path.path)));
        self.last_query_time = Instant::now();
    }

    fn confirm_selection(&mut self) -> DirPickerAction {
        if let Some(typed) = dir_search::resolve_query_path(self.modal.query()).filter(|path| path.is_dir()) {
            return self.select(Some(typed));
        }

        if let Some(path) = self
            .results
            .get(self.modal.selected_index())
            .map(|row| row.path.clone())
        {
            return self.select(Some(path));
        }

        if self.modal.query().is_empty() {
            return self.select(None);
        }

        DirPickerAction::None
    }

    fn take_purpose(&mut self) -> Option<DirPickerPurpose> {
        self.selected_purpose.take()
    }

    fn select(&mut self, path: Option<PathBuf>) -> DirPickerAction {
        match self.take_purpose() {
            Some(purpose) => DirPickerAction::Selected(path, Box::new(purpose)),
            None => DirPickerAction::Cancelled,
        }
    }
}

#[cfg(test)]
impl DirPicker {
    /// The purpose of the question, so a test sees which preset a press asked
    /// about before the person answers it.
    pub(crate) fn purpose(&self) -> Option<&DirPickerPurpose> {
        self.selected_purpose.as_ref()
    }
}

fn render_result_row(ui: &mut egui::Ui, width: f32, index: usize, row: &dir_search::DirRow, is_selected: bool) -> bool {
    let display = dir_search::abbreviate_home(&row.path);
    // The flag comes from the lookup of the row, so the frame reads no entry of
    // the directory: a home on a stalled mount does not freeze the picker.
    let is_project = row.is_project;

    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("dir_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT(), 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("dir_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;
    if is_project {
        ui.painter_at(row_rect)
            .circle_filled(Pos2::new(row_rect.min.x + 14.0, text_y), 3.0, theme::ACCENT());
    }

    let (dir_part, name_part) = split_path_display(&display);
    let text_x = row_rect.min.x + 26.0;

    if dir_part.is_empty() {
        ui.painter_at(row_rect).text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_CENTER,
            &display,
            egui::FontId::monospace(12.5),
            if is_selected { theme::FG() } else { theme::FG_SOFT() },
        );
    } else {
        let dir_end = ui.painter_at(row_rect).text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_CENTER,
            &dir_part,
            egui::FontId::monospace(12.5),
            theme::FG_DIM(),
        );
        ui.painter_at(row_rect).text(
            Pos2::new(dir_end.max.x, text_y),
            egui::Align2::LEFT_CENTER,
            &name_part,
            egui::FontId::monospace(12.5),
            if is_selected { theme::FG() } else { theme::FG_SOFT() },
        );
    }

    clicked
}

fn seed_query(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return String::new();
    };

    let mut query = dir_search::abbreviate_home(path);
    if !query.ends_with('/') {
        query.push('/');
    }
    query
}

#[cfg(test)]
mod tests {
    use std::fs;

    use horizon_core::{PanelKind, PanelResume, PresetConfig, WorkspaceId};
    use tempfile::TempDir;

    use super::{DirPicker, DirPickerAction, DirPickerPurpose, PickerModalState, seed_query};

    #[test]
    fn a_row_draws_without_reading_its_directory() {
        use crate::test_egui::DiscardTextures;

        // The frame of the picker runs under the guard in strict mode: a row
        // that looked for the markers of a project again would panic here.
        horizon_core::ui_thread::use_strict_mode();
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        let row = horizon_core::dir_search::DirRow {
            path: std::path::PathBuf::from("/nonexistent/hz-slim-row"),
            is_project: true,
        };
        let ctx = egui::Context::default();
        let _output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                super::render_result_row(ui, 300.0, 0, &row, true);
            })
            .discard_textures();
    }

    #[test]
    fn the_rows_of_a_lookup_carry_the_flag_of_their_directory() {
        let temp = TempDir::new().expect("temporary directory");
        let project = temp.path().join("project");
        let other = temp.path().join("other");
        fs::create_dir_all(project.join(".git")).expect("the markers of a project");
        fs::create_dir_all(&other).expect("a directory that is no project");

        let rows = horizon_core::dir_search::lookup(&format!("{}/", temp.path().display()));
        let flag = |path: &std::path::Path| {
            rows.iter()
                .find(|row| row.path == path)
                .map(|row| row.is_project)
                .expect("the directory of the row")
        };
        assert!(flag(&project), "a directory with the markers of a project is one");
        assert!(!flag(&other), "a plain directory is no project");
    }

    #[test]
    fn the_question_of_an_agent_preset_says_where_the_agent_works() {
        let question = |preset| {
            DirPicker::new(DirPickerPurpose::AddPanel {
                workspace_id: WorkspaceId(1),
                preset,
                canvas_pos: None,
            })
        };

        assert_eq!(
            question(agent_preset()).heading(),
            "Where should the agent work?",
            "the question names the folder that the agent works in"
        );
        assert_eq!(
            question(shell_preset()).heading(),
            "Select terminal directory",
            "a shell keeps the name of the terminal directory"
        );
    }

    fn agent_preset() -> PresetConfig {
        PresetConfig {
            name: "Codex".to_string(),
            alias: None,
            kind: PanelKind::Codex,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn seed_query_appends_trailing_separator() {
        assert_eq!(seed_query(Some(std::path::Path::new("/repo"))), "/repo/");
    }

    #[test]
    fn seed_query_is_empty_without_workspace_directory() {
        assert!(seed_query(None).is_empty());
    }

    #[test]
    fn confirm_selection_prefers_typed_directory_over_selected_child() {
        let temp_dir = TempDir::new().expect("temporary directory");
        let current_dir = temp_dir.path().join("current");
        let child_dir = current_dir.join("child");
        fs::create_dir_all(&child_dir).expect("child directory");

        let mut picker = DirPicker {
            modal: PickerModalState::new(format!("{}/", current_dir.display())),
            results: vec![horizon_core::dir_search::DirRow {
                path: child_dir,
                is_project: false,
            }],
            selected_purpose: Some(DirPickerPurpose::AddPanel {
                workspace_id: WorkspaceId(1),
                preset: shell_preset(),
                canvas_pos: None,
            }),
            search_rx: None,
            last_query_sent: String::new(),
            last_query_time: std::time::Instant::now(),
            initial_results_loaded: true,
        };

        let DirPickerAction::Selected(Some(path), _) = picker.confirm_selection() else {
            panic!("expected typed directory selection");
        };

        assert_eq!(path, current_dir);
    }

    fn shell_preset() -> PresetConfig {
        PresetConfig {
            name: "Shell".to_string(),
            alias: Some("sh".to_string()),
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }
}
