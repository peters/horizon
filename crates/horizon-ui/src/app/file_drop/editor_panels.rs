//! The panel of a dropped editor file. Its size comes from the file itself, so
//! the read that sizes it runs on the coordination worker, in drop order: a
//! large, slow or missing file never stalls the frame that took the drop, and
//! one panel per file opens on a later frame with the size that read computed.

use std::path::PathBuf;

use egui::Context;
use horizon_core::{PanelKind, PanelOptions, WorkspaceId};

use super::HorizonApp;
use crate::app::util::{EDITOR_PANEL_FALLBACK_SIZE, editor_panel_size_for_file};

/// A dropped editor file whose size read still runs, as the drop handler
/// resolved it. Only the size arrives later, so the entry keeps everything else
/// the panel needs.
struct PendingEditorDrop {
    id: u64,
    path: PathBuf,
    workspace_id: WorkspaceId,
    canvas_pos: Option<[f32; 2]>,
    name: Option<String>,
    kind: PanelKind,
    /// The context of the frame that took the drop. The reveal of the panel
    /// runs on a later frame, which has no context of its own here.
    ctx: Context,
}

/// The dropped editor files whose size read still runs.
#[derive(Default)]
pub(in crate::app) struct PendingEditorDrops {
    entries: Vec<PendingEditorDrop>,
    next_id: u64,
}

impl PendingEditorDrops {
    /// Records a dropped file and returns the id that its outcome settles, so a
    /// read that applies late cannot open another entry's panel.
    fn queue(
        &mut self,
        path: PathBuf,
        workspace_id: WorkspaceId,
        canvas_pos: Option<[f32; 2]>,
        name: Option<String>,
        ctx: Context,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(PendingEditorDrop {
            id,
            path,
            workspace_id,
            canvas_pos,
            name,
            kind: PanelKind::Editor,
            ctx,
        });
        id
    }

    fn take(&mut self, id: u64) -> Option<PendingEditorDrop> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        Some(self.entries.remove(index))
    }

    /// How many dropped files still wait for their size.
    #[cfg(test)]
    fn queued(&self) -> usize {
        self.entries.len()
    }
}

impl HorizonApp {
    /// Queues one dropped editor file: its read and size computation run on the
    /// coordination worker, after the files dropped before it, and its panel
    /// opens on the frame that applies its size.
    pub(super) fn queue_dropped_editor_file(
        &mut self,
        path: PathBuf,
        workspace_id: WorkspaceId,
        canvas_pos: Option<[f32; 2]>,
        name: Option<String>,
        ctx: &Context,
    ) {
        // The entry keeps the path the panel launches with; the job reads the
        // file from its own copy of it.
        let read = path.clone();
        let id = self
            .pending_editor_drops
            .queue(path, workspace_id, canvas_pos, name, ctx.clone());
        self.browser_create_host.io.then(
            move || editor_panel_size_for_file(&read),
            move |app, size| app.apply_editor_drop_size(id, size),
        );
    }

    /// Opens the panel of a dropped editor file once its size is known. A read
    /// that applies after the board it was dropped on was replaced opens no panel
    /// on the board that stands now, and a file that is gone, or that the worker
    /// could not read, keeps the size Horizon uses for such a file.
    fn apply_editor_drop_size(&mut self, id: u64, size: Option<[f32; 2]>) {
        let Some(dropped) = self.pending_editor_drops.take(id) else {
            return;
        };
        if self.browser_create_host.io.applying_stale() {
            return;
        }

        let options = PanelOptions {
            name: dropped.name,
            command: Some(dropped.path.display().to_string()),
            kind: dropped.kind,
            position: dropped.canvas_pos,
            size: Some(size.unwrap_or(EDITOR_PANEL_FALLBACK_SIZE)),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, dropped.workspace_id) {
            Ok(panel_id) => self.reveal_new_panel(&dropped.ctx, dropped.workspace_id, panel_id),
            Err(error) => tracing::error!("failed to create editor panel from dropped file: {error}"),
        }
        self.mark_runtime_dirty();
    }
}

#[cfg(test)]
mod tests;
