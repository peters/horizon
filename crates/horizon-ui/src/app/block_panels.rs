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
use horizon_core::{AgentStatus, BlockPanelState, Panel, PanelId, PanelKind, PresetConfig, preset_named};

use super::HorizonApp;
use super::actions::support::preset_panel_options;
use super::block_requests::{BlockAnswer, caller_unavailable};
use super::browser_requests::ActorPanel;

/// The panels each agent panel opened from a preset inside the window of
/// [`block_preset::PANEL_WINDOW`]. A panel that the person closed keeps its place
/// in the window, so a caller cannot reset its allowance by closing one.
#[derive(Default)]
pub(in crate::app) struct AgentPanels {
    opened: HashMap<String, Vec<Instant>>,
    /// The prompt line of each panel that an agent opened and that Horizon has not
    /// typed yet.
    prompts: Vec<PendingPrompt>,
}

/// One line that Horizon types into a new panel once its program has drawn, and
/// the instant after which Horizon stops waiting for that program.
pub(in crate::app) struct PendingPrompt {
    panel: PanelId,
    line: String,
    give_up_at: Instant,
}

/// How long Horizon waits for the program of a new panel to draw before it drops
/// the prompt line of a request.
pub(in crate::app) const PROMPT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

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

    /// Keeps the one line that `owner` asked Horizon to type into the panel at
    /// `panel`.
    fn hold_prompt(&mut self, panel: PanelId, line: &str, now: Instant) {
        self.prompts.push(PendingPrompt {
            panel,
            line: line.to_owned(),
            give_up_at: now + PROMPT_WINDOW,
        });
    }

    /// Whether a prompt line still waits for the program of its panel.
    pub(in crate::app) fn holds_prompts(&self) -> bool {
        !self.prompts.is_empty()
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
        let prompt = match asked.prompt {
            Some(line) => match block_preset::check_prompt(line) {
                Ok(line) => Some(line.to_owned()),
                Err(refusal) => return Some(block::Outcome::failed(refusal.code, &refusal.message)),
            },
            None => None,
        };
        if prompt.is_some() && !preset.kind.is_agent() {
            return Some(block::Outcome::failed(
                block_machines::INVALID_REQUEST,
                &format!(
                    "The preset {} starts no agent program, and a prompt line needs one: a shell or a command reads no sentence",
                    preset.name
                ),
            ));
        }
        let (root, request, ctx) = (root.map(Path::to_path_buf), request.clone(), ctx.clone());
        let opening = ctx.clone();
        self.block_claim.worker_job_started();
        self.browser_create_host.io.then(
            move || block_preset::check_directory_on_disk(&directory),
            move |app, checked| {
                app.block_claim.worker_job_finished();
                let outcome = match app
                    .request_caller(&request)
                    .and_then(|caller| caller.panel().ok_or_else(caller_unavailable))
                {
                    Err(refusal) => refusal,
                    Ok(actor) => match checked {
                        Some(Ok(path)) => app.open_panel(
                            &preset,
                            &path,
                            PanelText {
                                title: title.as_deref(),
                                prompt: prompt.as_deref(),
                            },
                            actor,
                            &request.actor,
                            &opening,
                        ),
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

    /// Answers `presets`: the presets that a panel of `open_panel` can be opened
    /// from, each with its name and its kind. It reads the settings in memory, so
    /// no frame waits for a file, and the answer holds no command, no argument, no
    /// alias and no folder: an agent learns which name to pass and which kind
    /// starts a program like its own.
    pub(super) fn answer_presets(&self) -> block::Outcome {
        block::Outcome::Presets {
            presets: self
                .template_config
                .presets
                .iter()
                .filter(|preset| preset.requires_workspace_cwd())
                .map(|preset| block::PresetSummary {
                    name: preset.name.clone(),
                    kind: preset_kind_name(preset.kind),
                })
                .collect(),
        }
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
        text: PanelText<'_>,
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
        options.name = Some(
            text.title
                .map_or_else(|| preset.name.clone(), BlockPanelState::normalize_title),
        );
        options.cwd = Some(directory.to_path_buf());
        options.opened_by = Some(mark);
        options.opened_by_actor = self.opener_actor(actor);
        super::actions::panels::normalize_new_panel_resume(&mut options);
        let focused = self.board.focused;
        let Ok(id) = self.create_agent_child_panel(options, actor.workspace_id, Some(actor.panel_id)) else {
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
        if let Some(line) = text.prompt {
            self.agent_opens.panels.hold_prompt(id, line, now);
            ctx.request_repaint();
        }
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

    /// Types the one line of each request into the panel of its preset, once the
    /// program of that panel has drawn and is idle. A panel that is gone, or one
    /// whose program draws nothing inside [`PROMPT_WINDOW`], drops its line.
    /// Returns whether the answer of a request needs a repaint.
    pub(super) fn write_ready_prompts(&mut self) -> bool {
        if !self.agent_opens.panels.holds_prompts() {
            return false;
        }
        let now = Instant::now();
        let mut kept: Vec<PendingPrompt> = Vec::new();
        let mut typed = false;
        for prompt in std::mem::take(&mut self.agent_opens.panels.prompts) {
            let Some(panel) = self.board.panel_mut(prompt.panel) else {
                continue;
            };
            let ready = prompt_ready(panel);
            let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
                continue;
            };
            if !ready {
                if now < prompt.give_up_at {
                    kept.push(prompt);
                }
                continue;
            }
            let mut bytes = crate::input::paste_bytes(&prompt.line, mode, true);
            bytes.push(b'\r');
            typed |= panel.write_input(&bytes);
        }
        self.agent_opens.panels.prompts = kept;
        typed
    }
}

/// The kind of a preset as the settings store it, such as `claude` or `shell`, for
/// the answer of `presets`. It is the stored token and not a display name, so an
/// agent compares it with the kind that its own panel reports.
fn preset_kind_name(kind: PanelKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(name)) => name,
        _ => kind.display_name().to_ascii_lowercase(),
    }
}

/// The text of one `open_panel` request: the title of the panel and the one line
/// that Horizon types into it when its program is ready.
#[derive(Clone, Copy, Default)]
pub(super) struct PanelText<'a> {
    pub title: Option<&'a str>,
    pub prompt: Option<&'a str>,
}

/// One `open_panel` request, as the dispatch reads it.
pub(super) struct OpenPanelFields<'a> {
    pub preset: &'a str,
    pub directory: &'a str,
    pub title: Option<&'a str>,
    /// One line that Horizon types into the panel once its program is ready.
    pub prompt: Option<&'a str>,
}

/// Whether the program of a new panel is ready for its one line: it runs, its
/// agent is at its prompt, and it drew at least one output of its own.
fn prompt_ready(panel: &Panel) -> bool {
    panel.kind.is_agent()
        && panel.agent_status() == AgentStatus::Idle
        && panel.drew_output()
        && panel.terminal().and_then(horizon_core::Terminal::child_pid).is_some()
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
