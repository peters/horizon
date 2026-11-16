use std::collections::{BTreeMap, HashSet};

use egui::text::{LayoutJob, TextFormat};
use egui::{Button, FontId, Ui};
use horizon_core::{PanelId, PanelKind, PresetConfig, WorkspaceId};

use crate::command_palette::{PanelEntry, PresetEntry, WorkspaceEntry};
use crate::theme;

use super::PresetPickerAction;
use crate::app::DetachedWorkspaceViewportState;

/// The options of a panel that a person creates from a preset. A browser panel
/// of this path follows the browser host that the settings name, and it writes no
/// `profile` field and names no engine, so the rule of
/// `horizon_core::browser::new_browser_panel` opens a page of the profile of that
/// host; every other caller of `PresetConfig::to_panel_options` keeps the backend
/// that it chose. A private panel is the one item of the panel menu that names a
/// profile.
pub(in crate::app) fn preset_panel_options(
    preset: &PresetConfig,
    browser_config: &horizon_core::browser::BrowserConfig,
) -> horizon_core::PanelOptions {
    let mut options = preset.to_panel_options(browser_config);
    options.follows_browser_host = matches!(preset.kind, PanelKind::Browser);
    if options.follows_browser_host {
        options.browser_request = Some(horizon_core::browser::BrowserPanelRequest::default());
    }
    options
}

pub(super) fn preset_picker_heading(target_workspace: Option<WorkspaceId>) -> &'static str {
    if target_workspace.is_some() {
        "New Terminal"
    } else {
        "New Workspace"
    }
}

/// The one line that a picker says above its agents while the workspace is on a
/// machine: an agent of such a workspace still starts on this computer, and only
/// its terminals run on the machine. Built when the menu opens, never per frame.
pub(in crate::app) fn placed_agent_hint(machine: Option<&str>) -> Option<String> {
    let machine = machine?;
    Some(format!("Agents start on this computer. Terminals start on {machine}."))
}

pub(super) fn render_grouped_preset_rows(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    presets: &[PresetConfig],
    agent_hint: Option<&str>,
    unavailable_reason: impl Fn(PanelKind) -> Option<&'static str>,
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    let mut any_group_rendered = false;

    for &category in &CATEGORY_ORDER {
        let mut group_started = false;

        for preset in presets {
            if preset_category(preset) != category {
                continue;
            }

            if !group_started {
                if any_group_rendered {
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                }
                if category == PresetCategory::Agent
                    && let Some(hint) = agent_hint
                {
                    ui.add(egui::Label::new(egui::RichText::new(hint).size(11.0).color(theme::FG_DIM())).wrap());
                    ui.add_space(2.0);
                }
                if category != PresetCategory::Shell {
                    ui.label(egui::RichText::new(category.label()).size(10.0).color(theme::FG_DIM()));
                    ui.add_space(1.0);
                }
                group_started = true;
            }

            let reason = unavailable_reason(preset.kind);
            let action = ui
                .add_enabled_ui(reason.is_none(), |ui| {
                    render_preset_picker_row(ui, target_workspace, canvas_pos, preset)
                })
                .inner;
            if let Some(action) = action {
                selected_action = Some(action);
            }
            if let Some(reason) = reason {
                ui.add(egui::Label::new(egui::RichText::new(reason).size(11.0).color(theme::FG_DIM())).wrap());
            }
        }

        if group_started {
            any_group_rendered = true;
        }
    }

    selected_action
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PresetCategory {
    Shell,
    Agent,
    Tool,
    Remote,
}

const CATEGORY_ORDER: [PresetCategory; 4] = [
    PresetCategory::Shell,
    PresetCategory::Agent,
    PresetCategory::Tool,
    PresetCategory::Remote,
];

impl PresetCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Agent => "Agents",
            Self::Tool => "Tools",
            Self::Remote => "Remote",
        }
    }
}

fn preset_ssh_connection(preset: &PresetConfig) -> Option<&horizon_core::SshConnection> {
    if preset.kind == PanelKind::Ssh {
        preset.ssh_connection.as_ref()
    } else {
        None
    }
}

fn preset_category(preset: &PresetConfig) -> PresetCategory {
    if preset.kind == PanelKind::Ssh {
        PresetCategory::Remote
    } else if preset.kind.is_agent() {
        PresetCategory::Agent
    } else if matches!(preset.kind, PanelKind::Shell) {
        PresetCategory::Shell
    } else {
        PresetCategory::Tool
    }
}

fn preset_button_label(preset: &PresetConfig) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &preset.name,
        0.0,
        TextFormat {
            font_id: FontId::proportional(12.5),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    if let Some(alias) = &preset.alias {
        job.append(
            &format!("  {alias}"),
            0.0,
            TextFormat {
                font_id: FontId::monospace(10.0),
                color: theme::FG_DIM(),
                ..Default::default()
            },
        );
    }
    job
}

fn render_preset_picker_row(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    match target_workspace {
        Some(workspace_id) => render_panel_preset_picker_row(ui, workspace_id, canvas_pos, preset),
        None => render_workspace_preset_picker_row(ui, canvas_pos, preset),
    }
}

fn render_panel_preset_picker_row(
    ui: &mut Ui,
    workspace_id: WorkspaceId,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    ui.horizontal(|ui| {
        if ui.add(Button::new(preset_button_label(preset)).frame(false)).clicked() {
            selected_action = Some(PresetPickerAction::CreatePanel {
                workspace_id,
                preset: preset.clone(),
                canvas_pos: Some(canvas_pos),
            });
        }

        let dir_text = egui::RichText::new("Dir").size(11.0).color(theme::FG_DIM());
        if ui.add(Button::new(dir_text).frame(false)).clicked() {
            selected_action = Some(PresetPickerAction::ChooseDirectory {
                workspace_id,
                preset: preset.clone(),
                canvas_pos: Some(canvas_pos),
            });
        }
    });
    selected_action
}

fn render_workspace_preset_picker_row(
    ui: &mut Ui,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    if !ui.add(Button::new(preset_button_label(preset)).frame(false)).clicked() {
        return None;
    }

    Some(if preset.requires_workspace_cwd() {
        PresetPickerAction::CreateWorkspace {
            canvas_pos,
            preset: preset.clone(),
        }
    } else {
        PresetPickerAction::CreateWorkspaceDirect {
            canvas_pos,
            preset: preset.clone(),
        }
    })
}

pub(super) fn detached_workspace_ids(
    board: &horizon_core::Board,
    detached_workspaces: &BTreeMap<String, DetachedWorkspaceViewportState>,
) -> HashSet<WorkspaceId> {
    detached_workspaces
        .keys()
        .filter_map(|local_id| board.workspace_id_by_local_id(local_id))
        .collect()
}

// A detached workspace paints its own panels inside its own viewport, so a
// panel from one can never be fullscreened in the root window: both passes run
// in the same frame and would reflow the one PTY to two different grid sizes.
pub(super) fn fullscreen_panel_is_renderable(
    board: &horizon_core::Board,
    detached_workspaces: &BTreeMap<String, DetachedWorkspaceViewportState>,
    panel_id: PanelId,
) -> bool {
    board.panel(panel_id).is_some_and(|panel| {
        board
            .workspace(panel.workspace_id)
            .is_some_and(|workspace| !detached_workspaces.contains_key(&workspace.local_id))
    })
}

pub(super) fn command_palette_workspace_entries(
    board: &horizon_core::Board,
    detached_workspace_ids: &HashSet<WorkspaceId>,
    active_workspace: Option<WorkspaceId>,
) -> Vec<WorkspaceEntry> {
    board
        .workspaces
        .iter()
        .filter(|workspace| !detached_workspace_ids.contains(&workspace.id))
        .map(|workspace| WorkspaceEntry {
            id: workspace.id,
            name: workspace.name.clone(),
            color: theme::workspace_accent(workspace.color_idx),
            panel_count: workspace.panels.len(),
            is_active: active_workspace == Some(workspace.id),
        })
        .collect()
}

pub(super) fn command_palette_panel_entries(
    board: &horizon_core::Board,
    detached_workspace_ids: &HashSet<WorkspaceId>,
) -> Vec<PanelEntry> {
    board
        .panels
        .iter()
        .filter(|panel| !detached_workspace_ids.contains(&panel.workspace_id))
        .map(|panel| {
            let workspace_name = board
                .workspace(panel.workspace_id)
                .map_or_else(String::new, |workspace| workspace.name.clone());
            PanelEntry {
                id: panel.id,
                title: panel.display_title().into_owned(),
                workspace_name,
                cwd: panel.launch_cwd.as_ref().map(|path| path.display().to_string()),
            }
        })
        .collect()
}

/// The presets whose agent program this computer does not have.
///
/// The search reads folders, so it runs on the worker when the list of the
/// presets opens, and the list draws unmarked until its answer is here.
#[derive(Default)]
pub(in crate::app) struct PresetPrograms {
    /// The presets without their program, by index of the settings.
    pub(in crate::app) missing: Vec<usize>,
}

/// The presets of `programs` whose program no folder of `PATH` holds. The search
/// reads folders, so only a worker calls it.
pub(in crate::app) fn missing_programs(programs: &[(usize, String)]) -> Vec<usize> {
    programs
        .iter()
        .filter(|(_, program)| horizon_core::setup::find_program(program, None).is_none())
        .map(|(index, _)| *index)
        .collect()
}

pub(super) fn command_palette_preset_entries(presets: &[PresetConfig], missing: &[usize]) -> Vec<PresetEntry> {
    presets
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            let ssh_connection = preset_ssh_connection(preset);
            let mut keywords = vec![preset.kind.display_name().to_ascii_lowercase()];
            if let Some(alias) = &preset.alias {
                keywords.push(alias.clone());
            }
            if let Some(connection) = ssh_connection {
                keywords.push(connection.host.clone());
                if let Some(user) = &connection.user {
                    keywords.push(user.clone());
                }
            }

            let detail = if let Some(connection) = ssh_connection {
                connection.display_label()
            } else if let Some(alias) = &preset.alias {
                format!("{}  {}", preset.kind.display_name(), alias)
            } else {
                preset.kind.display_name().to_string()
            };

            PresetEntry {
                index,
                label: preset.name.clone(),
                detail,
                keywords,
                not_installed: missing.contains(&index),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelResume, PresetConfig, SshConnection};

    use super::{
        PresetCategory, command_palette_preset_entries, missing_programs, preset_category, preset_panel_options,
    };

    #[test]
    fn the_search_marks_the_agent_presets_whose_program_is_not_here() {
        let folder = tempfile::tempdir().expect("a folder of the test");
        horizon_core::stand_in_program_in(folder.path(), "claude", "exit 0\n").expect("the program of a test");
        let program = |name: &str| folder.path().join(name).display().to_string();
        let programs = vec![
            (0, program("sh")),
            (1, program("codex")),
            (2, program("claude")),
            (3, program("gemini")),
        ];

        assert_eq!(
            missing_programs(&programs),
            vec![0, 1, 3],
            "only the program that is a file of this computer is found"
        );
    }

    fn browser_preset() -> PresetConfig {
        PresetConfig {
            name: "Browser".to_string(),
            alias: Some("web".to_string()),
            kind: PanelKind::Browser,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn a_browser_panel_of_the_picker_follows_the_named_browser_host() {
        let options = preset_panel_options(&browser_preset(), &horizon_core::browser::BrowserConfig::default());
        assert_eq!(options.kind, PanelKind::Browser);
        assert!(
            options.follows_browser_host,
            "the panel that a person picks opens on the browser host of the settings"
        );
        assert_eq!(
            options.browser_request,
            Some(horizon_core::browser::BrowserPanelRequest::default()),
            "the panel that a person picks writes no profile field and names no engine, so the rule opens a page of \
             the profile of the host"
        );
        assert!(
            options.browser_session_id.is_none(),
            "the rule decides the profile of the page, so the preset writes none"
        );

        let shell = preset_panel_options(
            &shell_preset_with_stale_ssh_metadata(),
            &horizon_core::browser::BrowserConfig::default(),
        );
        assert!(
            !shell.follows_browser_host,
            "a panel that is not a browser asks for no browser host"
        );
        assert!(
            shell.browser_session_id.is_none(),
            "a terminal joins no browser profile"
        );
    }

    fn shell_preset_with_stale_ssh_metadata() -> PresetConfig {
        PresetConfig {
            name: "Shell".to_string(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: Some(SshConnection {
                host: "prod-api".to_string(),
                user: Some("deploy".to_string()),
                ..SshConnection::default()
            }),
        }
    }

    /// A card of presets of every kind, as the picker groups them.
    fn presets_of_every_kind() -> Vec<PresetConfig> {
        let mut shell = shell_preset_with_stale_ssh_metadata();
        shell.ssh_connection = None;
        let mut agent = shell.clone();
        agent.name = "Codex".into();
        agent.kind = PanelKind::Codex;
        vec![shell, agent]
    }

    fn painted_lines(agent_hint: Option<&str>) -> Vec<String> {
        use crate::test_egui::DiscardTextures;
        use egui::{Rect, Vec2};
        let ctx = egui::Context::default();
        let presets = presets_of_every_kind();
        let workspace = horizon_core::Board::new().create_workspace("Fixture");
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(640.0, 480.0))),
                ..Default::default()
            },
            |ui| {
                super::render_grouped_preset_rows(ui, Some(workspace), [0.0, 0.0], &presets, agent_hint, |_| None);
            },
        )
        .discard_textures()
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect()
    }

    #[test]
    fn the_line_above_the_agents_names_the_machine_of_their_workspace() {
        assert_eq!(super::placed_agent_hint(None), None, "this computer adds no line");
        assert_eq!(
            super::placed_agent_hint(Some("build-1")).as_deref(),
            Some("Agents start on this computer. Terminals start on build-1.")
        );
    }

    #[test]
    fn the_picker_draws_the_line_above_the_agent_entries_only() {
        let hint = "Agents start on this computer. Terminals start on build-1.";
        let with_hint = painted_lines(Some(hint));
        assert!(with_hint.iter().any(|text| text == hint), "{with_hint:?}");
        let agents = with_hint
            .iter()
            .position(|text| text == "Agents")
            .expect("the agent group has its caption");
        let line = with_hint
            .iter()
            .position(|text| text == hint)
            .expect("the line is drawn");
        assert!(line < agents, "the line stands above the agent entries: {with_hint:?}");
        let shell = with_hint
            .iter()
            .position(|text| text == "Shell")
            .expect("the shell row is drawn");
        assert!(
            shell < line,
            "the line belongs to the agents, not the shell: {with_hint:?}"
        );

        let without = painted_lines(None);
        assert!(
            !without
                .iter()
                .any(|text| text.contains("Agents start on this computer"))
        );
    }

    #[test]
    fn preset_category_ignores_stale_ssh_metadata_for_non_ssh_presets() {
        assert!(matches!(
            preset_category(&shell_preset_with_stale_ssh_metadata()),
            PresetCategory::Shell
        ));
    }

    #[test]
    fn command_palette_preset_entries_ignore_stale_ssh_metadata_for_non_ssh_presets() {
        let entries = command_palette_preset_entries(&[shell_preset_with_stale_ssh_metadata()], &[]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail, "Shell");
        assert!(!entries[0].keywords.iter().any(|keyword| keyword == "prod-api"));
        assert!(!entries[0].keywords.iter().any(|keyword| keyword == "deploy"));
    }
    #[test]
    fn unavailable_preset_displays_reason_and_cannot_choose_panel_or_directory() {
        use crate::test_egui::DiscardTextures;
        use egui::{Event, PointerButton, Pos2, Rect, Vec2};
        let ctx = egui::Context::default();
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "Unavailable agent".into();
        preset.kind = PanelKind::Codex;
        let workspace = horizon_core::Board::new().create_workspace("Fixture");
        let frame = |events| {
            let mut action = None;
            let output = ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        action = super::render_grouped_preset_rows(
                            ui,
                            Some(workspace),
                            [0.0, 0.0],
                            std::slice::from_ref(&preset),
                            None,
                            |_| Some("Agent is disabled by this workspace preset"),
                        );
                    },
                )
                .discard_textures();
            assert!(action.is_none());
            output
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::Shape::Text(text) => Some((
                        text.galley.text().to_string(),
                        text.pos + text.galley.rect.center().to_vec2(),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let texts = frame(vec![]);
        assert!(
            texts
                .iter()
                .any(|(text, _)| text == "Agent is disabled by this workspace preset")
        );
        for label in ["Unavailable agent", "Dir"] {
            let pos = texts.iter().find(|(text, _)| text == label).unwrap().1;
            frame(vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ]);
            frame(vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }]);
        }
    }
}
