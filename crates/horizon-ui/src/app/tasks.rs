use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{
    GitHubRepoContext, GitHubWorkItemKind, PanelKind, PanelOptions, ResolvedGitHubWorkItem, TaskPanelStatus, TaskRole,
    TaskWaitStatus, TaskWorkspaceBinding, WorkspaceLayout, discover_repo_context, new_local_id,
    refresh_pull_request_status, resolve_work_item_input,
};

use crate::github_work_item_overlay::{GitHubWorkItemOverlay, GitHubWorkItemOverlayAction};

use super::{HorizonApp, TaskStatusRefresh};

const TASK_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

impl HorizonApp {
    pub(super) fn open_github_work_item_overlay(&mut self, kind: GitHubWorkItemKind) {
        self.github_work_item_overlay = Some(GitHubWorkItemOverlay::new(kind));
    }

    pub(super) fn render_github_work_item_overlay(&mut self, ctx: &Context) {
        let repo_context = self.active_repo_context();
        let repo_hint = repo_context.as_ref().map(|context| context.repo.slug());
        let Some(overlay) = self.github_work_item_overlay.as_mut() else {
            return;
        };

        let action = overlay.show(ctx, repo_hint.as_deref(), self.github_work_item_resolving);
        match action {
            GitHubWorkItemOverlayAction::None => {}
            GitHubWorkItemOverlayAction::Cancelled => self.dismiss_github_work_item_overlay(ctx),
            GitHubWorkItemOverlayAction::Submit(query) => {
                let Some(repo_context) = repo_context else {
                    overlay.set_error("No active local GitHub repo found in the current workspace.".to_string());
                    return;
                };
                if query.is_empty() {
                    overlay.set_error("Enter a GitHub reference first.".to_string());
                    return;
                }
                let kind = overlay.kind();
                self.github_work_item_resolve_rx = Some(Self::spawn_work_item_resolution(kind, query, repo_context));
                self.github_work_item_resolving = true;
            }
        }
    }

    pub(super) fn poll_github_work_item_resolution(&mut self, ctx: &Context) {
        let Some(rx) = self.github_work_item_resolve_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(result) => {
                self.github_work_item_resolving = false;
                match result {
                    Ok(resolved) => {
                        self.dismiss_github_work_item_overlay(ctx);
                        self.create_task_workspace(ctx, &resolved);
                    }
                    Err(error) => {
                        if let Some(overlay) = self.github_work_item_overlay.as_mut() {
                            overlay.set_error(error.to_string());
                        }
                    }
                }
            }
            Err(TryRecvError::Empty) => {
                self.github_work_item_resolve_rx = Some(rx);
            }
            Err(TryRecvError::Disconnected) => {
                self.github_work_item_resolving = false;
                if let Some(overlay) = self.github_work_item_overlay.as_mut() {
                    overlay.set_error("GitHub resolution worker disconnected.".to_string());
                }
            }
        }
    }

    pub(super) fn poll_task_status_refresh(&mut self) {
        self.poll_task_status_refresh_result();
        self.maybe_start_task_status_refresh();
    }

    pub(super) fn update_task_workspace_branch(
        &mut self,
        workspace_id: horizon_core::WorkspaceId,
        branch: Option<&str>,
    ) {
        for panel_id in self
            .board
            .workspace(workspace_id)
            .map(|workspace| workspace.panels.clone())
            .unwrap_or_default()
        {
            if let Some(panel) = self.board.panel_mut(panel_id)
                && panel.task_role().is_some()
            {
                panel.task_status.branch = branch.map(str::to_owned);
            }
        }
    }

    fn active_repo_context(&self) -> Option<GitHubRepoContext> {
        let mut candidates = Vec::new();
        if let Some(panel_id) = self.board.focused
            && let Some(panel) = self.board.panel(panel_id)
        {
            if let Some(cwd) = panel.launch_cwd.as_ref() {
                candidates.push(cwd.clone());
            }
            if let Some(cwd) = panel.terminal().and_then(horizon_core::Terminal::current_cwd) {
                candidates.push(cwd);
            }
        }
        if let Some(workspace_id) = self.board.active_workspace
            && let Some(workspace) = self.board.workspace(workspace_id)
            && let Some(cwd) = workspace.cwd.as_ref()
        {
            candidates.push(cwd.clone());
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd);
        }

        candidates
            .into_iter()
            .find_map(|candidate| discover_repo_context(&candidate).ok())
    }

    fn dismiss_github_work_item_overlay(&mut self, ctx: &Context) {
        self.github_work_item_overlay = None;
        self.github_work_item_resolving = false;
        self.github_work_item_resolve_rx = None;
        ctx.memory_mut(egui::Memory::stop_text_input);
    }

    fn spawn_work_item_resolution(
        kind: GitHubWorkItemKind,
        query: String,
        repo_context: GitHubRepoContext,
    ) -> Receiver<horizon_core::Result<ResolvedGitHubWorkItem>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(resolve_work_item_input(kind, &query, &repo_context));
        });
        rx
    }

    fn create_task_workspace(&mut self, ctx: &Context, resolved: &ResolvedGitHubWorkItem) {
        let task_id = new_local_id();
        let workspace_id = self.create_workspace_visible(ctx, &resolved.workspace_name);
        if let Some(workspace) = self.board.workspace_mut(workspace_id) {
            workspace.cwd = Some(resolved.repo_root.clone());
            workspace.task_binding = Some(TaskWorkspaceBinding {
                task_id: task_id.clone(),
                work_item: resolved.work_item.clone(),
                repo_root: resolved.repo_root.display().to_string(),
            });
        }

        let mut created_panels = Vec::new();
        for role in [TaskRole::Research, TaskRole::Implement, TaskRole::Review] {
            let options = self.task_panel_options(role, resolved);
            match self.create_panel_with_options(options, workspace_id) {
                Ok(panel_id) => created_panels.push(panel_id),
                Err(error) => {
                    tracing::error!(%error, role = role.label(), "failed creating task panel");
                    self.close_workspace_panels(workspace_id);
                    return;
                }
            }
        }

        self.board.arrange_workspace(workspace_id, WorkspaceLayout::Columns);
        if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
            self.focus_workspace_bounds(ctx, min, max, true);
        }
        if let Some(&panel_id) = created_panels.last() {
            self.board.focus(panel_id);
        }
        self.last_task_status_refresh = None;
        self.mark_runtime_dirty();
    }

    fn task_panel_options(&self, role: TaskRole, resolved: &ResolvedGitHubWorkItem) -> PanelOptions {
        let preferred_kind = match role {
            TaskRole::Research | TaskRole::Review => PanelKind::Claude,
            TaskRole::Implement => PanelKind::Codex,
        };
        let mut options = self
            .presets
            .iter()
            .find(|preset| preset.kind == preferred_kind)
            .map_or_else(
                || PanelOptions {
                    kind: preferred_kind,
                    ..PanelOptions::default()
                },
                horizon_core::PresetConfig::to_panel_options,
            );
        options.name = Some(format!("{} · {}", role.label(), resolved.work_item.label()));
        options.cwd = Some(resolved.repo_root.clone());
        options.task_role = Some(role);
        options.task_status = TaskPanelStatus {
            branch: resolved.branch.clone(),
            pr_state: resolved.pr_state.clone(),
            wait_status: TaskWaitStatus::Running,
        };
        options
    }

    fn maybe_start_task_status_refresh(&mut self) {
        if self.task_status_refresh_in_flight {
            return;
        }

        let should_refresh = self
            .last_task_status_refresh
            .is_none_or(|timestamp| timestamp.elapsed() >= TASK_STATUS_REFRESH_INTERVAL);
        if !should_refresh {
            return;
        }

        let queries = self.collect_task_status_queries();
        if queries.is_empty() {
            return;
        }

        self.task_status_refresh_rx = Some(Self::spawn_task_status_refresh(queries));
        self.task_status_refresh_in_flight = true;
    }

    fn collect_task_status_queries(
        &self,
    ) -> Vec<(
        horizon_core::WorkspaceId,
        horizon_core::GitHubWorkItemRef,
        Option<String>,
    )> {
        self.board
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let binding = workspace.task_binding.as_ref()?;
                let branch = workspace.panels.iter().find_map(|panel_id| {
                    self.board
                        .panel(*panel_id)
                        .and_then(|panel| panel.task_status().and_then(|s| s.branch.clone()))
                });
                Some((workspace.id, binding.work_item.clone(), branch))
            })
            .collect()
    }

    fn spawn_task_status_refresh(
        queries: Vec<(
            horizon_core::WorkspaceId,
            horizon_core::GitHubWorkItemRef,
            Option<String>,
        )>,
    ) -> Receiver<Vec<TaskStatusRefresh>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let refreshes = queries
                .into_iter()
                .filter_map(|(workspace_id, work_item, branch)| {
                    refresh_pull_request_status(&work_item, branch.as_deref())
                        .ok()
                        .map(|pr_state| TaskStatusRefresh { workspace_id, pr_state })
                })
                .collect();
            let _ = tx.send(refreshes);
        });
        rx
    }

    fn poll_task_status_refresh_result(&mut self) {
        let Some(rx) = self.task_status_refresh_rx.take() else {
            return;
        };

        match rx.try_recv() {
            Ok(refreshes) => {
                self.task_status_refresh_in_flight = false;
                self.last_task_status_refresh = Some(Instant::now());
                let mut changed = false;
                for refresh in refreshes {
                    changed |= self.apply_task_status_refresh(&refresh);
                }
                if changed {
                    self.mark_runtime_dirty();
                }
            }
            Err(TryRecvError::Empty) => {
                self.task_status_refresh_rx = Some(rx);
            }
            Err(TryRecvError::Disconnected) => {
                self.task_status_refresh_in_flight = false;
                self.last_task_status_refresh = Some(Instant::now());
            }
        }
    }

    fn apply_task_status_refresh(&mut self, refresh: &TaskStatusRefresh) -> bool {
        let panel_ids = self
            .board
            .workspace(refresh.workspace_id)
            .map(|workspace| workspace.panels.clone())
            .unwrap_or_default();
        let mut changed = false;
        for panel_id in panel_ids {
            if let Some(panel) = self.board.panel_mut(panel_id)
                && panel.task_role().is_some()
                && panel.task_status.pr_state != refresh.pr_state
            {
                panel.task_status.pr_state = refresh.pr_state.clone();
                changed = true;
            }
        }
        changed
    }
}
