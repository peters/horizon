//! The **Speech** tab of the Settings window.
//!
//! The tab holds the hotkey, the recorder line, the choice of the transcriber with
//! its fields, the state of the named secret of the key, and one **Try** button.
//! It says plainly that a take goes to the service of the endpoint, and it says
//! whether this computer holds a value for the name of the key. It shows no value.

use egui::Ui;
use horizon_core::named_secrets::{NamedSecretState, NamedSecretWorkbench};
use horizon_core::speech::SpeechPlatform;
use horizon_core::{Config, TranscriberKind};

use crate::theme;

use crate::app::speech::NOT_SET;

/// The height of the box of the text of one take of the **Try** button.
const TRY_ANSWER_HEIGHT: f32 = 64.0;

/// The size of the lines that the tab adds under a field.
const HINT_SIZE: f32 = 11.0;

/// The examples of a language hint, as the field shows them while it is empty.
pub(super) const LANGUAGE_EXAMPLES: &str = "no, en";

/// The line under the language field: what an empty hint means.
pub(super) const EMPTY_LANGUAGE: &str = "An empty language lets the service detect the language of each take.";

/// What the tab says about the shortcut of a retry, and about the keeps.
pub(super) const KEPT_TAKE_LINE: &str = "A take that the service did not answer stays on this computer for one retry: at most the last three takes, for \
     one day, private to this account. Shift and the hotkey send the last kept take again.";

/// What the tab asks of the window after it drew.
#[derive(Clone, Copy, Default)]
pub(super) struct SpeechTabOutcome {
    /// The draft changed.
    pub(super) changed: bool,
    /// The person pressed **Try**, so the window runs one take.
    pub(super) try_take: bool,
    /// The person asked for the kept take to be sent again.
    pub(super) retry_take: bool,
}

/// What a tab asks the window to do with speech input after it drew.
#[derive(Clone, Copy, Default)]
pub(super) struct SpeechRequest {
    pub(super) try_take: bool,
    pub(super) retry_take: bool,
}

/// The lines that the tab reads from the state of the window.
pub(super) struct SpeechView<'a> {
    pub(super) answer: Option<&'a Result<String, String>>,
    pub(super) running: bool,
    /// The sentence of a take that could not be sent and stays for one retry.
    pub(super) kept: Option<&'a str>,
    pub(super) secret: NamedSecretState,
}

pub(super) fn render(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &NamedSecretWorkbench,
    view: &SpeechView<'_>,
) -> SpeechTabOutcome {
    let mut outcome = SpeechTabOutcome::default();
    super::section_heading(ui, "Speech input");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Hold the hotkey to record. A take goes to the service of the endpoint, or to the command that prints \
             the text, and no speech engine runs in Horizon. Horizon runs the recorder of this computer and stops \
             it with SIGINT, and with SIGTERM when the recorder does not end on that. Escape cancels a take that \
             records.",
        );
        ui.add_space(8.0);
        outcome.changed |= render_hotkey(ui, config);
        ui.add_space(6.0);
        outcome.changed |= render_recorder(ui, config);
    });

    ui.add_space(10.0);
    super::section_heading(ui, "Transcriber");
    super::section_card(ui, |ui| {
        outcome.changed |= render_kind(ui, config);
        ui.add_space(8.0);
        match config.speech.transcriber.kind {
            TranscriberKind::Service => {
                outcome.changed |= render_service(ui, config, workbench, view);
            }
            TranscriberKind::Command => {
                outcome.changed |= render_command(ui, config);
            }
        }
    });

    ui.add_space(10.0);
    super::section_heading(ui, "Try");
    super::section_card(ui, |ui| {
        match config.speech.off_reason() {
            None => {
                if ui.button("Record two seconds").clicked() {
                    outcome.try_take = true;
                }
                if view.running {
                    ui.add_space(4.0);
                    super::dim_label(ui, "Recording for two seconds…");
                }
                ui.add_space(6.0);
                if let Some(sentence) = view.kept {
                    super::dim_label(ui, sentence);
                    if ui.button("Send the last kept take again").clicked() {
                        outcome.retry_take = true;
                    }
                } else {
                    super::dim_label(ui, KEPT_TAKE_LINE);
                }
            }
            Some(reason) => super::dim_label(ui, &format!("{NOT_SET} {reason}")),
        }
        outcome.changed |= render_answer(ui, view);
        super::validation_notice(
            ui,
            config.speech.validate(&config.shortcuts.resolve().unwrap_or_default()),
        );
    });
    outcome
}

fn render_hotkey(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    egui::Grid::new("settings_speech_hotkey")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Hotkey").color(theme::FG_SOFT()).size(12.0));
            changed = super::text_field(ui, &mut config.speech.hotkey, "F9", "settings_speech_hotkey");
            ui.end_row();
        });
    super::dim_label(
        ui,
        "One shortcut, such as F9 or Ctrl+Alt+S. A bare letter or arrow is refused, because a terminal needs it, \
         and so is a key of another shortcut of Horizon.",
    );
    changed
}

fn render_recorder(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    egui::Grid::new("settings_speech_recorder")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Recorder").color(theme::FG_SOFT()).size(12.0));
            changed = super::text_field(
                ui,
                &mut config.speech.recorder,
                "rec --rate 16000 {file}",
                "settings_speech_recorder",
            );
            ui.end_row();
        });
    let platform = SpeechPlatform::current();
    let recipe = match platform {
        SpeechPlatform::Linux => "On Linux, PipeWire records: pw-record --rate 16000 --channels 1 --format s16 {file}.",
        SpeechPlatform::MacOs => {
            "macOS has no recorder of its own. Install one, and write it here: ffmpeg -f avfoundation -i :default \
             -ac 1 -ar 16000 -y {file}, or rec -c 1 -r 16000 {file}."
        }
    };
    super::dim_label(
        ui,
        &format!(
            "One command line with {{file}}, which the take file replaces. Horizon runs the program itself and \
             never a shell. {recipe}"
        ),
    );
    super::dim_label(
        ui,
        &format!("The default of {} is {}.", platform.name(), platform.default_recorder()),
    );
    changed
}

fn render_kind(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Transcriber").color(theme::FG_SOFT()).size(12.0));
        for kind in TranscriberKind::ALL {
            if ui
                .selectable_label(config.speech.transcriber.kind == *kind, kind.label())
                .clicked()
                && config.speech.transcriber.kind != *kind
            {
                config.speech.transcriber.kind = *kind;
                changed = true;
            }
        }
    });
    changed
}

fn render_service(ui: &mut Ui, config: &mut Config, workbench: &NamedSecretWorkbench, view: &SpeechView<'_>) -> bool {
    let mut changed = false;
    egui::Grid::new("settings_speech_service")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Endpoint").color(theme::FG_SOFT()).size(12.0));
            changed |= super::text_field(
                ui,
                &mut config.speech.transcriber.endpoint,
                "https://api.openai.com/v1/audio/transcriptions",
                "settings_speech_endpoint",
            );
            ui.end_row();

            ui.label(egui::RichText::new("Model").color(theme::FG_SOFT()).size(12.0));
            changed |= super::text_field(
                ui,
                &mut config.speech.transcriber.model,
                "gpt-transcribe",
                "settings_speech_model",
            );
            ui.end_row();

            ui.label(egui::RichText::new("Language").color(theme::FG_SOFT()).size(12.0));
            changed |= super::text_field(
                ui,
                &mut config.speech.transcriber.language,
                LANGUAGE_EXAMPLES,
                "settings_speech_language",
            );
            ui.end_row();

            ui.label(egui::RichText::new("Key secret").color(theme::FG_SOFT()).size(12.0));
            changed |= super::text_field(
                ui,
                &mut config.speech.transcriber.key_secret,
                "SPEECH_API_KEY",
                "settings_speech_key",
            );
            ui.end_row();
        });
    super::dim_label(
        ui,
        "The endpoint is https, or http on an address of this computer for a stand-in service. The recording of a \
         take goes to the host of the endpoint, so read its host before you keep it.",
    );
    super::dim_label(ui, EMPTY_LANGUAGE);
    let state = if view.secret == NamedSecretState::Checking {
        workbench.state_of(config.speech.transcriber.key_secret.trim())
    } else {
        view.secret
    };
    let name = config.speech.transcriber.key_secret.trim();
    let (text, color) = state_line(name, state);
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).color(color).size(HINT_SIZE).strong());
    super::dim_label(
        ui,
        "The key is a named secret of this computer, so its value lives in the secrets file and reaches one \
         Authorization header. Store it once in the Secrets tab. A name that starts with OPENAI belongs to the \
         tools of Horizon, so the default is SPEECH_API_KEY.",
    );
    changed
}

fn render_command(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    egui::Grid::new("settings_speech_command")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Command").color(theme::FG_SOFT()).size(12.0));
            changed |= super::text_field(
                ui,
                &mut config.speech.transcriber.command,
                "my-transcriber {file}",
                "settings_speech_command_line",
            );
            ui.end_row();
        });
    super::dim_label(
        ui,
        "One command line with {file}. Its standard output is the text, its bound is 30 seconds, and it gets the \
         environment of Horizon and no named secret. An empty line keeps speech input off.",
    );
    changed
}

fn render_answer(ui: &mut Ui, view: &SpeechView<'_>) -> bool {
    let Some(answer) = view.answer else {
        return false;
    };
    ui.add_space(8.0);
    match answer {
        Ok(text) => {
            ui.label(
                egui::RichText::new("The take held:")
                    .color(theme::FG_SOFT())
                    .size(HINT_SIZE),
            );
            egui::ScrollArea::vertical()
                .max_height(TRY_ANSWER_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text.as_str())
                                .monospace()
                                .size(12.0)
                                .color(theme::FG()),
                        )
                        .wrap(),
                    );
                });
        }
        Err(message) => {
            ui.label(
                egui::RichText::new(message.as_str())
                    .color(theme::PALETTE_RED())
                    .size(12.0),
            );
        }
    }
    false
}

fn state_line(name: &str, state: NamedSecretState) -> (String, egui::Color32) {
    match state {
        NamedSecretState::Present => (format!("{name}: stored on this computer."), theme::PALETTE_GREEN()),
        NamedSecretState::NoValue => (format!("{name}: no value on this computer."), theme::PALETTE_RED()),
        NamedSecretState::Unavailable => (
            format!("{name}: the secrets file of this computer cannot be read."),
            theme::PALETTE_RED(),
        ),
        NamedSecretState::Checking => (format!("{name}: checking this computer."), theme::FG_DIM()),
    }
}
