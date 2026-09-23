//! Header controls: the SSH/VNC mode toggle and the destination workspace picker.
use egui::{Button, CornerRadius, FontId, Popup, PopupKind, RichText, Ui, Vec2};

use super::{RemoteConnectMode, WorkspaceChoice, WorkspaceOption};
use crate::theme;

const MODE_BUTTON_SIZE: Vec2 = Vec2::new(44.0, 22.0);
const DESTINATION_WIDTH: f32 = 190.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DestinationEntry {
    pub(super) choice: WorkspaceChoice,
    pub(super) label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DestinationPickerAction {
    None,
    /// Make the selected existing workspace the configured default.
    SetDefault,
}

/// The configured default first, then every other workspace by name. A
/// workspace already named like the default is reachable through that entry.
pub(super) fn destination_entries(workspaces: &[WorkspaceOption], default_workspace: &str) -> Vec<DestinationEntry> {
    let default_exists = workspaces.iter().any(|workspace| workspace.name == default_workspace);
    let mut entries = vec![DestinationEntry {
        choice: WorkspaceChoice::Default,
        label: if default_exists {
            default_workspace.to_string()
        } else {
            format!("{default_workspace} (new)")
        },
    }];
    entries.extend(
        workspaces
            .iter()
            .filter(|workspace| workspace.name != default_workspace)
            .map(|workspace| DestinationEntry {
                choice: WorkspaceChoice::Existing(workspace.id),
                label: workspace.name.clone(),
            }),
    );
    entries
}

/// A closed workspace can no longer be a destination; fall back to the default.
pub(super) fn normalize_destination(destination: &mut WorkspaceChoice, entries: &[DestinationEntry]) {
    if !entries.iter().any(|entry| entry.choice == *destination) {
        *destination = WorkspaceChoice::Default;
    }
}

pub(super) fn render_mode_toggle(ui: &mut Ui, mode: &mut RemoteConnectMode) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 2.0;
    for candidate in [RemoteConnectMode::Ssh, RemoteConnectMode::Vnc] {
        let selected = *mode == candidate;
        let (fill, color) = if selected {
            (theme::ACCENT(), theme::BG())
        } else {
            (theme::alpha(theme::PANEL_BG_ALT(), 170), theme::FG_SOFT())
        };
        let response = ui
            .add(
                Button::new(
                    RichText::new(candidate.label())
                        .font(FontId::proportional(11.5))
                        .strong()
                        .color(color),
                )
                .fill(fill)
                .corner_radius(CornerRadius::same(6))
                .min_size(MODE_BUTTON_SIZE),
            )
            .on_hover_text(candidate.hint());
        if response.clicked() && !selected {
            *mode = candidate;
            changed = true;
        }
    }
    changed
}

pub(super) fn render_destination_picker(
    ui: &mut Ui,
    destination: &mut WorkspaceChoice,
    entries: &[DestinationEntry],
) -> DestinationPickerAction {
    let mut action = DestinationPickerAction::None;
    if *destination != WorkspaceChoice::Default
        && ui
            .add(
                Button::new(
                    RichText::new("Set default")
                        .font(FontId::proportional(11.0))
                        .color(theme::FG_SOFT()),
                )
                .corner_radius(CornerRadius::same(6)),
            )
            .on_hover_text("Send future remote sessions to this workspace unless another is picked")
            .clicked()
    {
        action = DestinationPickerAction::SetDefault;
    }
    let selected = entries
        .iter()
        .find(|entry| entry.choice == *destination)
        .or_else(|| entries.first())
        .map_or("", |entry| entry.label.as_str());
    let button = ui
        .add(
            Button::new(
                RichText::new(format!("{selected}  \u{25be}"))
                    .font(FontId::proportional(12.0))
                    .color(theme::FG()),
            )
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(DESTINATION_WIDTH, 22.0)),
        )
        .on_hover_text("Workspace that receives the new session");
    // The overlay card sits on the Tooltip layer, so a default (Foreground)
    // popup would open underneath it.
    let popup_id = Popup::default_response_id(&button);
    let shown = Popup::menu(&button)
        .kind(PopupKind::Tooltip)
        .width(DESTINATION_WIDTH)
        .show(|ui| {
            for entry in entries {
                ui.selectable_value(destination, entry.choice.clone(), &entry.label);
            }
        })
        .is_some();
    if shown {
        // Same-order layers keep their previous relative order when both are
        // raised in one frame, and clicking the picker also raises the card.
        // A sublayer is spliced directly above its parent every pass instead.
        let popup_layer = egui::LayerId::new(PopupKind::Tooltip.order(), popup_id);
        ui.ctx()
            .memory_mut(|memory| memory.areas_mut().set_sublayer(button.layer_id, popup_layer));
    }
    ui.label(
        RichText::new("in")
            .font(FontId::proportional(11.5))
            .color(theme::FG_DIM()),
    );
    action
}

#[cfg(test)]
mod tests {
    use horizon_core::WorkspaceId;

    use super::*;

    fn workspaces(names: &[&str]) -> Vec<WorkspaceOption> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| WorkspaceOption {
                id: WorkspaceId(index as u64 + 1),
                name: (*name).to_string(),
            })
            .collect()
    }

    #[test]
    fn default_entry_comes_first_and_marks_a_missing_workspace_as_new() {
        let entries = destination_entries(&workspaces(&["Backend", "Ops"]), "Remote Sessions");
        assert_eq!(
            entries.iter().map(|entry| entry.label.as_str()).collect::<Vec<_>>(),
            vec!["Remote Sessions (new)", "Backend", "Ops"]
        );
        assert_eq!(entries[0].choice, WorkspaceChoice::Default);
        assert_eq!(entries[2].choice, WorkspaceChoice::Existing(WorkspaceId(2)));
    }

    #[test]
    fn an_existing_default_workspace_is_listed_once_through_the_default_entry() {
        let entries = destination_entries(&workspaces(&["Ops", "Remote Sessions"]), "Remote Sessions");
        assert_eq!(
            entries.iter().map(|entry| entry.label.as_str()).collect::<Vec<_>>(),
            vec!["Remote Sessions", "Ops"]
        );
    }

    #[test]
    fn a_closed_workspace_selection_falls_back_to_the_default() {
        let entries = destination_entries(&workspaces(&["Ops"]), "Remote Sessions");
        let mut destination = WorkspaceChoice::Existing(WorkspaceId(9));
        normalize_destination(&mut destination, &entries);
        assert_eq!(destination, WorkspaceChoice::Default);

        let mut kept = WorkspaceChoice::Existing(WorkspaceId(1));
        normalize_destination(&mut kept, &entries);
        assert_eq!(kept, WorkspaceChoice::Existing(WorkspaceId(1)));
    }
}
