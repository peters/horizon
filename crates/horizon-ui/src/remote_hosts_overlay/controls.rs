//! Overlay controls: the SSH/VNC mode toggle, the destination workspace
//! picker and the per-host context menu.
use egui::{Button, CornerRadius, FontId, Popup, PopupKind, Response, RichText, Ui, Vec2};

use super::{RemoteConnectMode, WorkspaceChoice, WorkspaceOption};
use crate::theme;

const MODE_BUTTON_SIZE: Vec2 = Vec2::new(44.0, 22.0);
const DESTINATION_WIDTH: f32 = 190.0;
const ROW_MENU_WIDTH: f32 = 200.0;

/// What a host row's context menu can do; the destination is the header's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowMenuChoice {
    Open(RemoteConnectMode),
    /// Store the host as a preset so any workspace can add it later.
    SaveShortcut(RemoteConnectMode),
}

impl RowMenuChoice {
    const ALL: [Self; 4] = [
        Self::Open(RemoteConnectMode::Ssh),
        Self::Open(RemoteConnectMode::Vnc),
        Self::SaveShortcut(RemoteConnectMode::Ssh),
        Self::SaveShortcut(RemoteConnectMode::Vnc),
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Open(RemoteConnectMode::Ssh) => "Open over SSH",
            Self::Open(RemoteConnectMode::Vnc) => "Open over VNC",
            Self::SaveShortcut(RemoteConnectMode::Ssh) => "Save SSH shortcut",
            Self::SaveShortcut(RemoteConnectMode::Vnc) => "Save VNC shortcut",
        }
    }

    const fn hint(self) -> &'static str {
        match self {
            Self::Open(_) => "Opens in the workspace picked in the header",
            Self::SaveShortcut(_) => "Adds a preset; the command palette can then create it in any workspace",
        }
    }
}

/// Show `popup` as a sublayer of the card it belongs to. Same-order layers
/// keep their previous relative order when both are raised in one frame, and
/// a click that opens a popup also raises the card; a sublayer is spliced
/// directly above its parent every pass instead.
fn show_card_popup<R>(
    ui: &Ui,
    popup: Popup<'_>,
    parent: egui::LayerId,
    content: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let popup_id = popup.get_id();
    let shown = popup.kind(PopupKind::Tooltip).show(content).map(|inner| inner.inner);
    if shown.is_some() {
        let popup_layer = egui::LayerId::new(PopupKind::Tooltip.order(), popup_id);
        ui.ctx()
            .memory_mut(|memory| memory.areas_mut().set_sublayer(parent, popup_layer));
    }
    shown
}

pub(super) fn render_row_menu(ui: &Ui, row: &Response) -> Option<RowMenuChoice> {
    show_card_popup(ui, Popup::context_menu(row).width(ROW_MENU_WIDTH), row.layer_id, |ui| {
        let mut picked = None;
        for choice in RowMenuChoice::ALL {
            if matches!(choice, RowMenuChoice::SaveShortcut(RemoteConnectMode::Ssh)) {
                ui.separator();
            }
            if ui.button(choice.label()).on_hover_text(choice.hint()).clicked() {
                picked = Some(choice);
                ui.close();
            }
        }
        picked
    })
    .flatten()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DestinationEntry {
    pub(super) choice: WorkspaceChoice,
    /// The workspace's real name, which is what a default persists.
    pub(super) name: String,
    /// Display text; duplicate names carry a running number here only.
    pub(super) label: String,
    /// Another listed workspace shares this name, so a default stored by
    /// name could not single this one out.
    pub(super) ambiguous: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DestinationPickerAction {
    None,
    /// Make the selected existing workspace the configured default.
    SetDefault,
}

/// The configured default first, then every other workspace by name. The
/// first workspace named like the default is what the default entry resolves
/// to, so only that one is folded into it; names are not unique, and later
/// duplicates stay selectable with a running number.
pub(super) fn destination_entries(workspaces: &[WorkspaceOption], default_workspace: &str) -> Vec<DestinationEntry> {
    let default_index = workspaces
        .iter()
        .position(|workspace| workspace.name == default_workspace);
    let mut entries = vec![DestinationEntry {
        choice: WorkspaceChoice::Default,
        name: default_workspace.to_string(),
        label: if default_index.is_some() {
            default_workspace.to_string()
        } else {
            format!("{default_workspace} (new)")
        },
        ambiguous: false,
    }];
    let mut seen: Vec<(&str, usize)> = vec![(default_workspace, 1)];
    for (index, workspace) in workspaces.iter().enumerate() {
        if Some(index) == default_index {
            continue;
        }
        let label = if let Some((_, count)) = seen.iter_mut().find(|(name, _)| *name == workspace.name) {
            *count += 1;
            format!("{} ({count})", workspace.name)
        } else {
            seen.push((workspace.name.as_str(), 1));
            workspace.name.clone()
        };
        entries.push(DestinationEntry {
            choice: WorkspaceChoice::Existing(workspace.id),
            name: workspace.name.clone(),
            label,
            ambiguous: false,
        });
    }
    for index in 1..entries.len() {
        let name = entries[index].name.clone();
        let shared = entries
            .iter()
            .enumerate()
            .any(|(other, entry)| other != index && entry.name == name);
        entries[index].ambiguous = shared;
    }
    entries
}

/// A closed workspace can no longer be a destination; fall back to the default.
pub(super) fn normalize_destination(destination: &mut WorkspaceChoice, entries: &[DestinationEntry]) {
    if !entries.iter().any(|entry| entry.choice == *destination) {
        *destination = WorkspaceChoice::Default;
    }
}

/// Move the selection one entry forward or back, wrapping around.
pub(super) fn cycle_destination(destination: &mut WorkspaceChoice, entries: &[DestinationEntry], forward: bool) {
    if entries.is_empty() {
        return;
    }
    let current = entries
        .iter()
        .position(|entry| entry.choice == *destination)
        .unwrap_or(0);
    let next = if forward {
        (current + 1) % entries.len()
    } else {
        (current + entries.len() - 1) % entries.len()
    };
    destination.clone_from(&entries[next].choice);
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
                .selected(selected)
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
    let selected_entry = entries.iter().find(|entry| entry.choice == *destination);
    if *destination != WorkspaceChoice::Default
        && selected_entry.is_some_and(|entry| !entry.ambiguous)
        && ui
            .add(
                Button::new(
                    RichText::new("Set default")
                        .font(FontId::proportional(11.0))
                        .color(theme::FG_SOFT()),
                )
                .corner_radius(CornerRadius::same(6)),
            )
            .on_hover_text("Send future remote sessions to this workspace unless another is picked (Alt+D)")
            .clicked()
    {
        action = DestinationPickerAction::SetDefault;
    }
    if selected_entry.is_some_and(|entry| entry.ambiguous) {
        ui.label(
            RichText::new("shares its name; cannot be the default")
                .font(FontId::proportional(11.0))
                .color(theme::FG_DIM()),
        )
        .on_hover_text("Defaults are stored by name; rename the workspace to make it the default");
    }
    let selected = selected_entry
        .or_else(|| entries.first())
        .map_or("", |entry| entry.label.as_str());
    // A fixed width with truncation keeps a long workspace name from pushing
    // the rest of the header around; the full name is in the tooltip.
    let button = ui
        .add_sized(
            Vec2::new(DESTINATION_WIDTH, 22.0),
            Button::new(
                RichText::new(format!("{selected}  \u{25be}"))
                    .font(FontId::proportional(12.0))
                    .color(theme::FG()),
            )
            .wrap_mode(egui::TextWrapMode::Truncate)
            .corner_radius(CornerRadius::same(6)),
        )
        .on_hover_text(format!(
            "{selected}\nWorkspace that receives the new session (Alt+\u{2191}/\u{2193} cycles)"
        ));
    // The overlay card sits on the Tooltip layer, so a default (Foreground)
    // popup would open underneath it.
    show_card_popup(
        ui,
        Popup::menu(&button).width(DESTINATION_WIDTH),
        button.layer_id,
        |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for entry in entries {
                ui.selectable_value(destination, entry.choice.clone(), &entry.label)
                    .on_hover_text(&entry.label);
            }
        },
    );
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

    #[test]
    fn row_menu_lists_open_and_save_for_both_modes() {
        assert_eq!(
            RowMenuChoice::ALL.map(RowMenuChoice::label),
            [
                "Open over SSH",
                "Open over VNC",
                "Save SSH shortcut",
                "Save VNC shortcut"
            ]
        );
    }

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
    fn duplicate_names_stay_selectable_with_a_running_number() {
        // Names are not unique; only the first default-named workspace folds
        // into the default entry, which is the one `Default` resolves to.
        let entries = destination_entries(
            &workspaces(&["Remote Sessions", "Ops", "Remote Sessions", "Ops"]),
            "Remote Sessions",
        );
        assert_eq!(
            entries.iter().map(|entry| entry.label.as_str()).collect::<Vec<_>>(),
            vec!["Remote Sessions", "Ops", "Remote Sessions (2)", "Ops (2)"]
        );
        assert_eq!(entries[2].choice, WorkspaceChoice::Existing(WorkspaceId(3)));
        assert_eq!(entries[3].choice, WorkspaceChoice::Existing(WorkspaceId(4)));
        assert_eq!(
            entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            vec!["Remote Sessions", "Ops", "Remote Sessions", "Ops"],
            "the running number is display text only"
        );
        assert_eq!(
            entries.iter().map(|entry| entry.ambiguous).collect::<Vec<_>>(),
            vec![false, true, true, true],
            "every workspace sharing a name is ambiguous as a default"
        );
        let unique = destination_entries(&workspaces(&["Ops"]), "Remote Sessions");
        assert!(unique.iter().all(|entry| !entry.ambiguous));
    }

    #[test]
    fn cycling_wraps_through_every_destination_in_both_directions() {
        let entries = destination_entries(&workspaces(&["Backend", "Ops"]), "Remote Sessions");
        let mut destination = WorkspaceChoice::Default;
        cycle_destination(&mut destination, &entries, true);
        assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(1)));
        cycle_destination(&mut destination, &entries, true);
        assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(2)));
        cycle_destination(&mut destination, &entries, true);
        assert_eq!(destination, WorkspaceChoice::Default);
        cycle_destination(&mut destination, &entries, false);
        assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(2)));
        cycle_destination(&mut destination, &[], false);
        assert_eq!(destination, WorkspaceChoice::Existing(WorkspaceId(2)));
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
