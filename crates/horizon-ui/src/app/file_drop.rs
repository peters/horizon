use std::collections::HashMap;
use std::path::Path;

use egui::{Context, Pos2, Rect};
use horizon_core::{PanelId, PanelKind, PanelOptions, WorkspaceId};

use crate::input;

use super::HorizonApp;
use super::util::editor_panel_size_for_file;

#[derive(Clone, Copy)]
enum FileDropScope {
    Root,
    Workspace(WorkspaceId),
}

impl HorizonApp {
    pub(super) fn handle_root_file_drop(&mut self, ctx: &Context) {
        let workspace_id = self
            .board
            .active_workspace
            .unwrap_or_else(|| self.ensure_workspace_visible(ctx));
        let fullscreen_panel = self
            .fullscreen_panel
            .filter(|panel_id| self.panel_is_in_root_viewport(*panel_id));
        self.handle_file_drop_for_viewport(
            ctx,
            self.canvas_rect(ctx),
            workspace_id,
            fullscreen_panel,
            FileDropScope::Root,
        );
    }

    pub(super) fn handle_workspace_file_drop(&mut self, ctx: &Context, workspace_id: WorkspaceId, canvas_rect: Rect) {
        let fullscreen_panel = self
            .fullscreen_panel
            .filter(|panel_id| self.panel_is_in_workspace(*panel_id, workspace_id));
        self.handle_file_drop_for_viewport(
            ctx,
            canvas_rect,
            workspace_id,
            fullscreen_panel,
            FileDropScope::Workspace(workspace_id),
        );
    }

    fn handle_file_drop_for_viewport(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        workspace_id: WorkspaceId,
        fullscreen_panel: Option<PanelId>,
        scope: FileDropScope,
    ) {
        let viewport_id = ctx.viewport_id();
        let (hovered, dropped, pointer_pos) = ctx.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input.raw.dropped_files.clone(),
                input.pointer.hover_pos().or(input.pointer.latest_pos()),
            )
        });

        if hovered && let Some(pos) = pointer_pos {
            self.file_hover_positions.insert(viewport_id, pos);
        }

        if dropped.is_empty() {
            return;
        }

        let screen_pos = self.file_hover_positions.remove(&viewport_id).or(pointer_pos);

        if let Some(panel_id) = self.terminal_drop_target(fullscreen_panel, screen_pos, scope)
            && self.paste_dropped_paths_into_terminal(panel_id, &dropped)
        {
            return;
        }

        self.open_dropped_editor_files(canvas_rect, workspace_id, screen_pos, &dropped);
    }

    fn panel_is_in_root_viewport(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .is_some_and(|panel| !self.workspace_is_detached(panel.workspace_id))
    }

    fn panel_is_in_workspace(&self, panel_id: PanelId, workspace_id: WorkspaceId) -> bool {
        self.board
            .panel(panel_id)
            .is_some_and(|panel| panel.workspace_id == workspace_id)
    }

    fn panel_is_in_scope(&self, panel_id: PanelId, scope: FileDropScope) -> bool {
        match scope {
            FileDropScope::Root => self.panel_is_in_root_viewport(panel_id),
            FileDropScope::Workspace(workspace_id) => self.panel_is_in_workspace(panel_id, workspace_id),
        }
    }

    fn terminal_drop_target(
        &self,
        fullscreen_panel: Option<PanelId>,
        screen_pos: Option<Pos2>,
        scope: FileDropScope,
    ) -> Option<PanelId> {
        let focused_terminal = self.board.focused.filter(|panel_id| {
            self.panel_is_in_scope(*panel_id, scope)
                && self
                    .board
                    .panel(*panel_id)
                    .is_some_and(|panel| panel.terminal().is_some())
        });

        select_terminal_drop_target(
            fullscreen_panel,
            screen_pos,
            focused_terminal,
            &self.panel_screen_order,
            &self.panel_screen_rects,
            |panel_id| {
                self.panel_is_in_scope(panel_id, scope)
                    && self
                        .board
                        .panel(panel_id)
                        .is_some_and(|panel| panel.terminal().is_some())
            },
        )
    }

    fn paste_dropped_paths_into_terminal(&mut self, panel_id: PanelId, dropped: &[egui::DroppedFile]) -> bool {
        let Some(payload) = format_dropped_paths_for_terminal(dropped) else {
            return false;
        };

        let did_paste = {
            let Some(panel) = self.board.panel_mut(panel_id) else {
                return false;
            };
            let Some(terminal) = panel.terminal_mut() else {
                return false;
            };

            terminal.clear_selection();
            let bytes = input::paste_bytes(&payload, terminal.mode(), true);
            terminal.write_input(&bytes);
            true
        };

        if did_paste {
            self.board.focus(panel_id);
        }

        did_paste
    }

    fn open_dropped_editor_files(
        &mut self,
        canvas_rect: Rect,
        workspace_id: WorkspaceId,
        screen_pos: Option<Pos2>,
        dropped: &[egui::DroppedFile],
    ) {
        let canvas_pos = screen_pos.map(|pos| self.screen_to_canvas(canvas_rect, pos));

        for file in dropped {
            let Some(path) = file.path.clone() else { continue };
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if !matches!(ext, "md" | "markdown" | "txt" | "mdx") {
                continue;
            }

            let options = PanelOptions {
                name: path.file_name().map(|name| name.to_string_lossy().to_string()),
                command: Some(path.display().to_string()),
                kind: PanelKind::Editor,
                position: canvas_pos.map(|pos| [pos.x, pos.y]),
                size: Some(editor_panel_size_for_file(&path)),
                ..PanelOptions::default()
            };
            if let Err(error) = self.create_panel_with_options(options, workspace_id) {
                tracing::error!("failed to create editor panel from dropped file: {error}");
            }
            self.mark_runtime_dirty();
        }
    }
}

fn select_terminal_drop_target(
    fullscreen_panel: Option<PanelId>,
    screen_pos: Option<Pos2>,
    focused_terminal: Option<PanelId>,
    panel_screen_order: &[PanelId],
    panel_screen_rects: &HashMap<PanelId, Rect>,
    mut is_terminal_panel: impl FnMut(PanelId) -> bool,
) -> Option<PanelId> {
    if let Some(panel_id) = fullscreen_panel {
        return is_terminal_panel(panel_id).then_some(panel_id);
    }

    if let Some(screen_pos) = screen_pos {
        return panel_screen_order.iter().rev().copied().find(|panel_id| {
            panel_screen_rects
                .get(panel_id)
                .is_some_and(|rect| rect.contains(screen_pos) && is_terminal_panel(*panel_id))
        });
    }

    if let Some(panel_id) = focused_terminal
        && is_terminal_panel(panel_id)
    {
        return Some(panel_id);
    }

    panel_screen_order
        .iter()
        .rev()
        .copied()
        .find(|panel_id| is_terminal_panel(*panel_id))
}

fn format_dropped_paths_for_terminal(dropped: &[egui::DroppedFile]) -> Option<String> {
    let mut formatted = Vec::new();

    for path in dropped.iter().filter_map(|file| file.path.as_deref()) {
        formatted.push(format_path_for_terminal(path));
    }

    if formatted.is_empty() {
        return None;
    }

    let mut payload = formatted.join(" ");
    payload.push(' ');
    Some(payload)
}

fn format_path_for_terminal(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        format_windows_path(&path)
    } else {
        format_posix_path(&path)
    }
}

fn format_posix_path(path: &str) -> String {
    if !path_needs_posix_quotes(path) {
        return path.to_string();
    }

    format!("'{}'", path.replace('\'', "'\"'\"'"))
}

fn path_needs_posix_quotes(path: &str) -> bool {
    path.is_empty()
        || path
            .chars()
            .any(|ch| !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '/' | '.' | '_' | '-'))
}

fn format_windows_path(path: &str) -> String {
    if !path_needs_windows_quotes(path) {
        return path.to_string();
    }

    let trailing_backslashes = path.chars().rev().take_while(|ch| *ch == '\\').count();
    format!("\"{path}{}\"", "\\".repeat(trailing_backslashes))
}

fn path_needs_windows_quotes(path: &str) -> bool {
    path.is_empty()
        || path
            .chars()
            .any(|ch| matches!(ch, ' ' | '\t' | '&' | '|' | '<' | '>' | '^' | '(' | ')' | '%' | '!'))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use egui::{Pos2, Rect};
    use horizon_core::PanelId;

    use super::{format_dropped_paths_for_terminal, select_terminal_drop_target};

    #[test]
    fn formatting_keeps_safe_paths_unquoted() {
        let dropped = [egui::DroppedFile {
            path: Some("/tmp/image.png".into()),
            ..egui::DroppedFile::default()
        }];

        let payload = format_dropped_paths_for_terminal(&dropped).expect("payload");

        assert_eq!(payload, "/tmp/image.png ");
    }

    #[test]
    fn formatting_shell_quotes_posix_paths_when_needed() {
        if cfg!(windows) {
            return;
        }

        let dropped = [egui::DroppedFile {
            path: Some(PathBuf::from("/tmp/hello world's.png")),
            ..egui::DroppedFile::default()
        }];

        let payload = format_dropped_paths_for_terminal(&dropped).expect("payload");

        assert_eq!(payload, "'/tmp/hello world'\"'\"'s.png' ");
    }

    #[test]
    fn target_selection_prefers_topmost_terminal_panel() {
        let bottom = PanelId(1);
        let middle = PanelId(2);
        let top = PanelId(3);
        let order = vec![bottom, middle, top];
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0));
        let rects = HashMap::from([(bottom, rect), (middle, rect), (top, rect)]);

        let target = select_terminal_drop_target(None, Some(Pos2::new(50.0, 50.0)), None, &order, &rects, |panel_id| {
            panel_id == middle || panel_id == top
        });

        assert_eq!(target, Some(top));
    }

    #[test]
    fn target_selection_prefers_fullscreen_terminal_without_rects() {
        let fullscreen = PanelId(7);

        let target = select_terminal_drop_target(Some(fullscreen), None, None, &[], &HashMap::new(), |panel_id| {
            panel_id == fullscreen
        });

        assert_eq!(target, Some(fullscreen));
    }

    #[test]
    fn target_selection_ignores_stale_root_rects_for_non_terminal_fullscreen_panels() {
        let fullscreen = PanelId(9);
        let stale = PanelId(5);
        let order = vec![stale];
        let rects = HashMap::from([(stale, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0)))]);

        let target = select_terminal_drop_target(
            Some(fullscreen),
            Some(Pos2::new(20.0, 20.0)),
            None,
            &order,
            &rects,
            |panel_id| panel_id == stale,
        );

        assert_eq!(target, None);
    }

    #[test]
    fn target_selection_falls_back_to_focused_terminal_when_drop_position_is_missing() {
        let focused = PanelId(4);
        let other = PanelId(5);
        let order = vec![other, focused];

        let target = select_terminal_drop_target(None, None, Some(focused), &order, &HashMap::new(), |panel_id| {
            panel_id == focused || panel_id == other
        });

        assert_eq!(target, Some(focused));
    }

    #[test]
    fn target_selection_falls_back_to_topmost_terminal_when_no_position_or_focus_exists() {
        let bottom = PanelId(1);
        let top = PanelId(2);
        let order = vec![bottom, top];

        let target = select_terminal_drop_target(None, None, None, &order, &HashMap::new(), |panel_id| {
            panel_id == bottom || panel_id == top
        });

        assert_eq!(target, Some(top));
    }

    #[test]
    fn target_selection_does_not_fall_back_when_drop_position_is_outside_terminals() {
        let focused = PanelId(4);
        let order = vec![focused];
        let rects = HashMap::from([(
            focused,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0)),
        )]);

        let target = select_terminal_drop_target(
            None,
            Some(Pos2::new(180.0, 180.0)),
            Some(focused),
            &order,
            &rects,
            |panel_id| panel_id == focused,
        );

        assert_eq!(target, None);
    }
}
