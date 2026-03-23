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

#[derive(Clone, Copy)]
struct TerminalDropRects<'a> {
    panels: &'a HashMap<PanelId, Rect>,
    terminal_bodies: &'a HashMap<PanelId, Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalDropHit {
    Ignore,
    Panel,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileDropHighlight {
    Panel(PanelId),
    Workspace(WorkspaceId),
}

impl HorizonApp {
    pub(super) fn handle_root_file_drop(&mut self, ctx: &Context) {
        let workspace_id = self.board.active_workspace;
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
            Some(workspace_id),
            fullscreen_panel,
            FileDropScope::Workspace(workspace_id),
        );
    }

    fn handle_file_drop_for_viewport(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        workspace_id: Option<WorkspaceId>,
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

        // Only query the native cursor during an active drag; on Linux this
        // opens an X11 connection each call, so skip it when idle.
        let native_pointer_pos = if hovered || !dropped.is_empty() {
            native_file_drop_position(ctx)
        } else {
            None
        };
        let hover_pos = native_pointer_pos.or(pointer_pos);

        if hovered && let Some(pos) = hover_pos {
            self.file_hover_positions.insert(viewport_id, pos);
        }

        self.file_drop_highlight = if hovered {
            self.compute_file_drop_highlight(ctx, hover_pos, fullscreen_panel, scope)
        } else {
            None
        };

        if dropped.is_empty() {
            return;
        }

        let screen_pos = native_pointer_pos
            .or_else(|| self.file_hover_positions.remove(&viewport_id))
            .or(pointer_pos);
        let allow_terminal_fallback = !dropped
            .iter()
            .filter_map(|file| file.path.as_deref())
            .any(is_editor_drop_path);

        if let Some(panel_id) = self.terminal_drop_target(fullscreen_panel, screen_pos, scope, allow_terminal_fallback)
        {
            if self.maybe_start_ssh_file_drop(panel_id, &dropped, viewport_id) {
                return;
            }
            if self.paste_dropped_paths_into_terminal(panel_id, &dropped) {
                return;
            }
        }

        self.open_dropped_editor_files(ctx, canvas_rect, workspace_id, screen_pos, &dropped);
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
        allow_fallback: bool,
    ) -> Option<PanelId> {
        let focused_terminal = self
            .board
            .focused
            .filter(|panel_id| !matches!(self.panel_drop_hit(*panel_id, scope), TerminalDropHit::Ignore));

        select_terminal_drop_target(
            fullscreen_panel,
            screen_pos,
            focused_terminal,
            &self.panel_screen_order,
            TerminalDropRects {
                panels: &self.panel_screen_rects,
                terminal_bodies: &self.terminal_body_screen_rects,
            },
            allow_fallback,
            |panel_id| self.panel_drop_hit(panel_id, scope),
        )
    }

    fn panel_drop_hit(&self, panel_id: PanelId, scope: FileDropScope) -> TerminalDropHit {
        if !self.panel_is_in_scope(panel_id, scope) {
            return TerminalDropHit::Ignore;
        }

        match self.board.panel(panel_id) {
            Some(panel) if panel.kind == PanelKind::Ssh => TerminalDropHit::Panel,
            Some(panel) if panel.terminal().is_some() => TerminalDropHit::Body,
            _ => TerminalDropHit::Ignore,
        }
    }

    fn compute_file_drop_highlight(
        &self,
        ctx: &Context,
        hover_pos: Option<Pos2>,
        fullscreen_panel: Option<PanelId>,
        scope: FileDropScope,
    ) -> Option<FileDropHighlight> {
        let hover_pos = hover_pos?;

        let all_editor = ctx.input(|input| {
            let paths: Vec<_> = input.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect();
            !paths.is_empty() && paths.iter().all(|p| is_editor_drop_path(p))
        });

        // Fullscreen: only highlight if it is a droppable panel.
        if let Some(panel_id) = fullscreen_panel {
            return match self.panel_drop_hit(panel_id, scope) {
                TerminalDropHit::Panel => Some(FileDropHighlight::Panel(panel_id)),
                TerminalDropHit::Body if !all_editor => Some(FileDropHighlight::Panel(panel_id)),
                _ => None,
            };
        }

        // Find topmost panel under cursor.
        let hovered_panel = self
            .panel_screen_order
            .iter()
            .rev()
            .copied()
            .find(|id| self.panel_screen_rects.get(id).is_some_and(|r| r.contains(hover_pos)));

        if let Some(panel_id) = hovered_panel {
            match self.panel_drop_hit(panel_id, scope) {
                TerminalDropHit::Panel => return Some(FileDropHighlight::Panel(panel_id)),
                TerminalDropHit::Body if !all_editor => return Some(FileDropHighlight::Panel(panel_id)),
                _ => {}
            }
        }

        // Editor files over a workspace highlight the workspace.
        if all_editor {
            for &(ws_id, rect) in self.workspace_screen_rects.iter().rev() {
                if rect.contains(hover_pos) {
                    return Some(FileDropHighlight::Workspace(ws_id));
                }
            }
        }

        None
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
        ctx: &Context,
        canvas_rect: Rect,
        mut workspace_id: Option<WorkspaceId>,
        screen_pos: Option<Pos2>,
        dropped: &[egui::DroppedFile],
    ) {
        let canvas_pos = screen_pos.map(|pos| self.screen_to_canvas(canvas_rect, pos));

        for file in dropped {
            let Some(path) = file.path.clone() else { continue };
            if !is_editor_drop_path(&path) {
                continue;
            }

            let workspace_id = *workspace_id.get_or_insert_with(|| self.ensure_workspace_visible(ctx));
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

fn is_editor_drop_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "markdown" | "txt" | "mdx")
    )
}

fn select_terminal_drop_target(
    fullscreen_panel: Option<PanelId>,
    screen_pos: Option<Pos2>,
    focused_terminal: Option<PanelId>,
    panel_screen_order: &[PanelId],
    rects: TerminalDropRects<'_>,
    allow_fallback: bool,
    mut panel_drop_hit: impl FnMut(PanelId) -> TerminalDropHit,
) -> Option<PanelId> {
    if let Some(panel_id) = fullscreen_panel {
        return (!matches!(panel_drop_hit(panel_id), TerminalDropHit::Ignore)).then_some(panel_id);
    }

    if let Some(screen_pos) = screen_pos {
        let hovered_panel = panel_screen_order
            .iter()
            .rev()
            .copied()
            .find(|panel_id| rects.panels.get(panel_id).is_some_and(|rect| rect.contains(screen_pos)));

        let panel_id = hovered_panel?;
        match panel_drop_hit(panel_id) {
            TerminalDropHit::Ignore => return None,
            TerminalDropHit::Panel => return Some(panel_id),
            TerminalDropHit::Body => {}
        }

        return rects
            .terminal_bodies
            .get(&panel_id)
            .is_some_and(|rect| rect.contains(screen_pos))
            .then_some(panel_id);
    }

    if !allow_fallback {
        return None;
    }

    if let Some(panel_id) = focused_terminal
        && !matches!(panel_drop_hit(panel_id), TerminalDropHit::Ignore)
    {
        return Some(panel_id);
    }

    panel_screen_order
        .iter()
        .rev()
        .copied()
        .find(|panel_id| !matches!(panel_drop_hit(*panel_id), TerminalDropHit::Ignore))
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

fn native_file_drop_position(ctx: &Context) -> Option<Pos2> {
    let inner_rect = ctx.input(|input| input.viewport().inner_rect)?;
    let global_pos = native_cursor_position()?;
    let local_pos = global_pos - inner_rect.min.to_vec2();

    Rect::from_min_size(Pos2::ZERO, inner_rect.size())
        .contains(local_pos)
        .then_some(local_pos)
}

fn native_cursor_position() -> Option<Pos2> {
    use egui::emath::Numeric as _;

    let (x, y) = horizon_cursor::cursor_position()?;
    Some(Pos2::new(f32::from_f64(f64::from(x)), f32::from_f64(f64::from(y))))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use egui::{Pos2, Rect};
    use horizon_core::PanelId;

    use super::{
        TerminalDropHit, TerminalDropRects, format_dropped_paths_for_terminal, is_editor_drop_path,
        native_cursor_position, select_terminal_drop_target,
    };

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
    fn editor_drop_detection_accepts_supported_extensions_only() {
        assert!(is_editor_drop_path(std::path::Path::new("/tmp/note.md")));
        assert!(is_editor_drop_path(std::path::Path::new("/tmp/draft.markdown")));
        assert!(is_editor_drop_path(std::path::Path::new("/tmp/readme.txt")));
        assert!(is_editor_drop_path(std::path::Path::new("/tmp/page.mdx")));
        assert!(!is_editor_drop_path(std::path::Path::new("/tmp/image.png")));
        assert!(!is_editor_drop_path(std::path::Path::new("/tmp/archive.tar.gz")));
    }

    fn hit_fn(ids: &[PanelId]) -> impl FnMut(PanelId) -> TerminalDropHit + '_ {
        move |panel_id| {
            if ids.contains(&panel_id) {
                TerminalDropHit::Body
            } else {
                TerminalDropHit::Ignore
            }
        }
    }

    #[test]
    fn target_selection_prefers_topmost_terminal_panel() {
        let bottom = PanelId(1);
        let middle = PanelId(2);
        let top = PanelId(3);
        let order = vec![bottom, middle, top];
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0));
        let rects = HashMap::from([(bottom, rect), (middle, rect), (top, rect)]);
        let body_rects = HashMap::from([(middle, rect), (top, rect)]);

        let target = select_terminal_drop_target(
            None,
            Some(Pos2::new(50.0, 50.0)),
            None,
            &order,
            TerminalDropRects {
                panels: &rects,
                terminal_bodies: &body_rects,
            },
            true,
            hit_fn(&[middle, top]),
        );

        assert_eq!(target, Some(top));
    }

    #[test]
    fn target_selection_prefers_fullscreen_terminal_without_rects() {
        let fullscreen = PanelId(7);

        let target = select_terminal_drop_target(
            Some(fullscreen),
            None,
            None,
            &[],
            TerminalDropRects {
                panels: &HashMap::new(),
                terminal_bodies: &HashMap::new(),
            },
            true,
            hit_fn(&[fullscreen]),
        );

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
            TerminalDropRects {
                panels: &rects,
                terminal_bodies: &HashMap::new(),
            },
            true,
            hit_fn(&[stale]),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn target_selection_falls_back_to_focused_terminal_when_drop_position_is_missing() {
        let focused = PanelId(4);
        let other = PanelId(5);
        let order = vec![other, focused];

        let target = select_terminal_drop_target(
            None,
            None,
            Some(focused),
            &order,
            TerminalDropRects {
                panels: &HashMap::new(),
                terminal_bodies: &HashMap::new(),
            },
            true,
            hit_fn(&[focused, other]),
        );

        assert_eq!(target, Some(focused));
    }

    #[test]
    fn target_selection_falls_back_to_topmost_terminal_when_no_position_or_focus_exists() {
        let bottom = PanelId(1);
        let top = PanelId(2);
        let order = vec![bottom, top];

        let target = select_terminal_drop_target(
            None,
            None,
            None,
            &order,
            TerminalDropRects {
                panels: &HashMap::new(),
                terminal_bodies: &HashMap::new(),
            },
            true,
            hit_fn(&[bottom, top]),
        );

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
            TerminalDropRects {
                panels: &rects,
                terminal_bodies: &rects,
            },
            true,
            hit_fn(&[focused]),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn target_selection_does_not_pick_underlying_terminal_beneath_non_terminal_panel() {
        let terminal = PanelId(9);
        let editor = PanelId(10);
        let order = vec![terminal, editor];
        let rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0));
        let panel_rects = HashMap::from([(terminal, rect), (editor, rect)]);
        let body_rects = HashMap::from([(terminal, rect)]);

        let target = select_terminal_drop_target(
            None,
            Some(Pos2::new(80.0, 80.0)),
            Some(terminal),
            &order,
            TerminalDropRects {
                panels: &panel_rects,
                terminal_bodies: &body_rects,
            },
            true,
            hit_fn(&[terminal]),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn target_selection_disables_terminal_fallback_for_editor_drops() {
        let terminal = PanelId(11);
        let order = vec![terminal];
        let panel_rects = HashMap::from([(
            terminal,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0)),
        )]);
        let body_rects = HashMap::from([(
            terminal,
            Rect::from_min_max(Pos2::new(20.0, 40.0), Pos2::new(180.0, 180.0)),
        )]);

        let target = select_terminal_drop_target(
            None,
            None,
            Some(terminal),
            &order,
            TerminalDropRects {
                panels: &panel_rects,
                terminal_bodies: &body_rects,
            },
            false,
            hit_fn(&[terminal]),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn target_selection_keeps_explicit_terminal_body_hits_for_editor_drops() {
        let terminal = PanelId(12);
        let order = vec![terminal];
        let panel_rects = HashMap::from([(
            terminal,
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0)),
        )]);
        let body_rects = HashMap::from([(
            terminal,
            Rect::from_min_max(Pos2::new(20.0, 40.0), Pos2::new(180.0, 180.0)),
        )]);

        let target = select_terminal_drop_target(
            None,
            Some(Pos2::new(60.0, 80.0)),
            Some(terminal),
            &order,
            TerminalDropRects {
                panels: &panel_rects,
                terminal_bodies: &body_rects,
            },
            false,
            hit_fn(&[terminal]),
        );

        assert_eq!(target, Some(terminal));
    }

    #[test]
    fn target_selection_accepts_ssh_panel_hits_outside_terminal_body_for_editor_drops() {
        let ssh = PanelId(13);
        let order = vec![ssh];
        let panel_rects = HashMap::from([(ssh, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 200.0)))]);
        let body_rects = HashMap::from([(ssh, Rect::from_min_max(Pos2::new(20.0, 40.0), Pos2::new(180.0, 180.0)))]);

        let target = select_terminal_drop_target(
            None,
            Some(Pos2::new(12.0, 12.0)),
            Some(ssh),
            &order,
            TerminalDropRects {
                panels: &panel_rects,
                terminal_bodies: &body_rects,
            },
            false,
            |panel_id| {
                if panel_id == ssh {
                    TerminalDropHit::Panel
                } else {
                    TerminalDropHit::Ignore
                }
            },
        );

        assert_eq!(target, Some(ssh));
    }

    #[test]
    fn native_cursor_position_is_queryable_when_supported() {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }

        let _ = native_cursor_position();
    }
}
