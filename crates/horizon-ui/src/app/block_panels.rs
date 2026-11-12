//! The panel of a preset that an agent asks Horizon to open for a person.
//!
//! `block_panel open_panel` names one preset of the person's settings and one
//! folder. Horizon takes the command, the arguments and the kind from the preset
//! and never from the request, so an agent cannot choose what runs: it chooses a
//! preset that the person saved and the folder that its task uses. The folder is
//! read on the coordination worker, because a frame reads no file, and the panel
//! opens on the frame that applies the answer. The panel goes beside the calling
//! agent panel, it does not take keyboard focus, and its titlebar carries the
//! mark `opened by <agent>`, which is Horizon's own text.
//!
//! After the panel exists it is the person's: no operation of `block_panel` reads
//! it, writes to it or closes it.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_preset;
use horizon_core::{BlockPanelState, PresetConfig, preset_named};

use super::HorizonApp;
use super::actions::support::preset_panel_options;
use super::block_requests::BlockAnswer;
use super::browser_requests::ActorPanel;

/// The panels each agent panel opened from a preset inside the window of
/// [`block_preset::PANEL_WINDOW`]. A panel that the person closed keeps its place
/// in the window, so a caller cannot reset its allowance by closing one.
#[derive(Default)]
pub(in crate::app) struct AgentPanels {
    opened: HashMap<String, Vec<Instant>>,
}

impl AgentPanels {
    /// The instants of the panels that `owner` opened inside the window at `now`.
    fn recent(&self, owner: &str, now: Instant) -> Vec<Instant> {
        self.opened
            .get(owner)
            .map(|opened| {
                opened
                    .iter()
                    .copied()
                    .filter(|at| now.saturating_duration_since(*at) < block_preset::PANEL_WINDOW)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The seconds that `owner` must wait before another panel, or `None` when it
    /// may open one now.
    pub(in crate::app) fn wait_seconds(&self, owner: &str, now: Instant) -> Option<u64> {
        let mut recent = self.recent(owner, now);
        if recent.len() < block_preset::MAX_PANELS_PER_OWNER {
            return None;
        }
        recent.sort_unstable();
        let oldest = recent.first().copied()?;
        let waited = now.saturating_duration_since(oldest);
        let left = block_preset::PANEL_WINDOW.saturating_sub(waited);
        Some(left.as_secs().max(1))
    }

    /// Counts one panel that `owner` opened now.
    pub(in crate::app) fn record(&mut self, owner: &str, now: Instant) {
        let recent = self.recent(owner, now);
        let opened = self.opened.entry(owner.to_owned()).or_default();
        opened.clear();
        opened.extend(recent);
        opened.push(now);
    }
}

impl HorizonApp {
    /// Answers `open_panel`: resolves the preset of the settings, checks the shape
    /// of its folder, and hands the read of that folder to the coordination
    /// worker. The panel opens on the frame that applies the answer, and this
    /// frame answers the caller nothing by itself.
    pub(super) fn answer_open_panel(
        &mut self,
        asked: &OpenPanelFields<'_>,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<block::Outcome> {
        let name = match block_preset::check_name(asked.preset) {
            Ok(name) => name,
            Err(refusal) => return Some(block::Outcome::failed(refusal.code, &refusal.message)),
        };
        let Some(preset) = preset_named(&self.template_config.presets, name).cloned() else {
            return Some(block::Outcome::failed(
                block_preset::UNKNOWN_PRESET,
                &block_preset::unknown_preset_message(&self.preset_names()),
            ));
        };
        if !preset.requires_workspace_cwd() {
            return Some(block::Outcome::failed(
                block_machines::INVALID_REQUEST,
                &format!(
                    "The preset {} opens no program of this computer, so it has no working directory: name a preset of a shell, a command or an agent",
                    preset.name
                ),
            ));
        }
        let directory = match block_preset::check_directory(asked.directory) {
            Ok(directory) => directory.to_owned(),
            Err(refusal) => return Some(block::Outcome::failed(refusal.code, &refusal.message)),
        };
        let title = asked.title.map(str::to_owned);
        let (root, request, ctx) = (root.map(Path::to_path_buf), request.clone(), ctx.clone());
        let opening = ctx.clone();
        self.block_claim.worker_job_started();
        self.browser_create_host.io.then(
            move || block_preset::check_directory_on_disk(&directory),
            move |app, checked| {
                app.block_claim.worker_job_finished();
                let outcome = match app.request_caller(&request) {
                    Err(refusal) => refusal,
                    Ok(actor) => match checked {
                        Some(Ok(path)) => {
                            app.open_panel(&preset, &path, title.as_deref(), actor, &request.actor, &opening)
                        }
                        Some(Err(refusal)) => block::Outcome::failed(refusal.code, &refusal.message),
                        None => worker_stopped(),
                    },
                };
                app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
                ctx.request_repaint();
            },
        );
        None
    }

    /// The names of the presets of the settings, each alias as `alias (name)`, for
    /// the answer of an unknown preset. It lists names only: a command of a preset
    /// is never in an answer.
    fn preset_names(&self) -> Vec<String> {
        self.template_config
            .presets
            .iter()
            .map(|preset| match &preset.alias {
                Some(alias) => format!("{} ({alias})", preset.name),
                None => preset.name.clone(),
            })
            .collect()
    }

    /// Opens the panel of one preset in `directory`, beside the calling agent
    /// panel, without taking keyboard focus. The preset is the person's: this call
    /// copies its command, its arguments and its kind into the panel, and it adds
    /// the mark of the agent.
    pub(super) fn open_panel(
        &mut self,
        preset: &PresetConfig,
        directory: &Path,
        title: Option<&str>,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> block::Outcome {
        let now = Instant::now();
        if let Some(wait) = self.agent_opens.panels.wait_seconds(owner, now) {
            return block::Outcome::failed(
                block_preset::RATE_LIMITED,
                &format!(
                    "This agent panel opened the maximum of {} panels in ten minutes; one more needs {wait} seconds",
                    block_preset::MAX_PANELS_PER_OWNER
                ),
            );
        }
        let mark = self.opener_mark(actor);
        let mut options = preset_panel_options(preset, &self.template_config.browser);
        options.name = Some(title.map_or_else(|| preset.name.clone(), BlockPanelState::normalize_title));
        options.cwd = Some(directory.to_path_buf());
        options.opened_by = Some(mark);
        super::actions::panels::normalize_new_panel_resume(&mut options);
        let focused = self.board.focused;
        let Ok(id) = self.create_agent_child_panel(options, actor.workspace_id, actor.panel_id) else {
            return block::Outcome::failed(
                block_preset::PANEL_UNAVAILABLE,
                "Horizon could not open the panel of the preset in this workspace",
            );
        };
        // A panel for the person must not steal keyboard input from the agent that
        // asked for it.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        self.agent_opens.panels.record(owner, now);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        let Some(panel) = self.board.panel(id) else {
            return block::Outcome::failed(
                block_preset::PANEL_UNAVAILABLE,
                "The new panel is not in this workspace",
            );
        };
        block::Outcome::Opened {
            panel_id: panel.local_id.clone(),
        }
    }
}

/// One `open_panel` request, as the dispatch reads it.
pub(super) struct OpenPanelFields<'a> {
    pub preset: &'a str,
    pub directory: &'a str,
    pub title: Option<&'a str>,
}

/// The answer of a worker job that ended before it answered.
fn worker_stopped() -> block::Outcome {
    block::Outcome::failed(
        block_preset::PANEL_UNAVAILABLE,
        "The Horizon worker that reads the directory of the panel stopped before it answered; ask again",
    )
}

#[cfg(test)]
mod tests;
