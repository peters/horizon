//! The editor panel of a spawn.
//!
//! A file panel opens at once: the panel of a saved file shows an empty buffer
//! until the worker that reads the text answers, and a panel of a path that does
//! not exist is a new document at that path.

use std::path::PathBuf;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::runtime_state::new_local_id;
use crate::workspace::WorkspaceId;

use super::super::{Panel, PanelId, PanelKind, PanelOptions};
use super::StaticPanelSeed;

/// The panel of a file whose text a worker reads. The panel opens at once with
/// an empty buffer and shows the text when the answer arrives.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a waiting panel answers the same result as the panel it stands for"
)]
pub(super) fn waiting_editor_panel(id: PanelId, workspace_id: WorkspaceId, mut opts: PanelOptions) -> Result<Panel> {
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    let command = opts.command.take();
    let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
    let mut seed = seed;
    let (title, has_custom_name) = seed.take_title(|| {
        command
            .as_deref()
            .and_then(|path| {
                PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "Markdown".to_string())
    });
    let editor = command.as_deref().map_or_else(MarkdownEditor::scratch, |path| {
        MarkdownEditor::waiting_for(PathBuf::from(path))
    });
    Ok(seed.into_panel(
        title,
        PanelKind::Editor,
        crate::editor::PanelContent::Editor(editor),
        command,
        None,
        has_custom_name,
    ))
}

pub(super) fn spawn_editor(mut seed: StaticPanelSeed, command: Option<String>) -> Result<Panel> {
    let editor = if let Some(ref path_str) = command {
        let path = PathBuf::from(path_str);
        if path.exists() {
            MarkdownEditor::open(path)?
        } else {
            let mut editor = MarkdownEditor::scratch();
            editor.file_path = Some(path);
            editor
        }
    } else {
        MarkdownEditor::scratch()
    };

    let (title, has_custom_name) = seed.take_title(|| {
        command
            .as_deref()
            .and_then(|path| {
                PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "Markdown".to_string())
    });

    tracing::info!("created editor panel '{}' (id={})", title, seed.id.0);

    Ok(seed.into_panel(
        title,
        PanelKind::Editor,
        PanelContent::Editor(editor),
        command,
        None,
        has_custom_name,
    ))
}
