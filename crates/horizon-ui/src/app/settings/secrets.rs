//! Secrets tab: the named secrets that every agent panel of this computer reads as
//! an environment variable.
//!
//! The tab holds the names, and the value of a name goes straight into the named
//! secret worker and then into the credential store. A value is never drawn again
//! after the person typed it, never written to the settings, and never copied out:
//! the store answers presence only, and [`NamedSecretState`] carries no value.

use egui::Ui;
use horizon_core::Config;
use horizon_core::named_secrets::{
    MAX_NAMED_SECRETS, MAX_VALUE_BYTES, NamedSecretError, NamedSecretState, NamedSecretWorkbench, SecretName,
    check_value,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::theme;

/// The length bound of one value field, in bytes as the rule counts it.
const VALUE_CHAR_LIMIT: usize = MAX_VALUE_BYTES;

/// The text buffers of the tab. Dropped with the settings editor, and every buffer
/// is overwritten before it goes.
#[derive(Default)]
pub(super) struct SecretInputs {
    /// The name that the person types for a new named secret.
    name: String,
    /// The value of that name, hidden, and scrubbed when it was sent.
    value: Zeroizing<String>,
    /// The row whose value the person is replacing, and its hidden value.
    replacing: Option<SecretName>,
    replacement: Zeroizing<String>,
    /// The row whose delete the person confirmed once.
    confirming: Option<SecretName>,
    /// The last refusal of a name or of a value, in the words of its rule.
    refusal: Option<String>,
    /// The number of names that the line under the list describes.
    count: usize,
    count_line: String,
    /// The names of the list that the last frame parsed, with their parsed form, so a
    /// frame that changes nothing parses nothing.
    parsed: Vec<(String, SecretName)>,
}

impl SecretInputs {
    /// The parsed names of `names`, from the last frame when the list did not change.
    /// The list is moved out and put back, so a frame allocates nothing.
    fn parsed_names(&mut self, names: &[String]) -> Vec<(String, SecretName)> {
        let same =
            self.parsed.len() == names.len() && self.parsed.iter().zip(names).all(|((drawn, _), name)| drawn == name);
        if same {
            return std::mem::take(&mut self.parsed);
        }
        names
            .iter()
            .filter_map(|text| SecretName::parse(text).ok().map(|name| (text.clone(), name)))
            .collect()
    }

    /// The line that counts the names of the list. It is written again only when the
    /// number of names changes, so a frame that changes nothing allocates nothing.
    fn count_text(&mut self, names: &[String]) -> &str {
        if self.count != names.len() {
            self.count = names.len();
            self.count_line = format!("{} of {MAX_NAMED_SECRETS} names", names.len());
        }
        &self.count_line
    }

    /// A draft that a person typed into the name field and the value field, for a
    /// test that proves the value is never drawn.
    #[cfg(test)]
    pub(super) fn with_typed(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: Zeroizing::new(value.to_owned()),
            ..Self::default()
        }
    }

    /// The name and the value that the person typed, for a test that reads them back.
    #[cfg(test)]
    pub(super) fn typed(&self) -> (String, String) {
        (self.name.clone(), self.value.to_string())
    }

    /// Forgets every value that the person typed, for a close or a save, so a draft
    /// never outlives the window that it was typed in.
    pub(super) fn scrub(&mut self) {
        self.name.zeroize();
        self.value.zeroize();
        self.replacement.zeroize();
        self.replacing = None;
        self.confirming = None;
        self.refusal = None;
    }

    /// Takes a typed value out of its buffer into a zeroizing copy.
    fn take(buffer: &mut Zeroizing<String>) -> Zeroizing<Vec<u8>> {
        let bytes = Zeroizing::new(buffer.as_bytes().to_vec());
        buffer.zeroize();
        bytes
    }
}

/// Draws the named secrets of this computer. Returns whether the draft settings
/// changed, which is a name that the person added or deleted.
pub(super) fn render(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &mut NamedSecretWorkbench,
    inputs: &mut SecretInputs,
) -> bool {
    let mut changed = false;
    super::section_heading(ui, "Secrets for agents");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "Each secret is one environment variable of every agent panel that Horizon starts from now on, on this \
             computer. A panel that is already running keeps the environment of its start. A terminal panel, and a \
             terminal on a machine over SSH, gets none. An agent, and every tool that it starts, can read the value \
             and send it, so give a tool a key of the smallest scope.",
        );
        ui.add_space(8.0);

        let names = inputs.parsed_names(&config.secrets.names);
        changed |= render_names(ui, &names, workbench, inputs, config);
        inputs.parsed = names;
        ui.add_space(8.0);
        let count = inputs.count_text(&config.secrets.names);
        ui.label(egui::RichText::new(count).color(theme::FG_DIM()).size(11.0));
        ui.add_space(6.0);
        changed |= render_add(ui, config, workbench, inputs);
        if let Some(refusal) = inputs.refusal.as_deref() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(refusal).color(theme::PALETTE_RED()).size(11.0));
        }
        if let Some(error) = workbench.take_failure() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("The credential store of this computer: {error}"))
                    .color(theme::PALETTE_RED())
                    .size(11.0),
            );
        }
    });
    changed
}

/// One row per name: its state on this computer, and the two actions that the person
/// can take on it. A name whose value this computer does not hold says so and is not
/// injected.
fn render_names(
    ui: &mut Ui,
    names: &[(String, SecretName)],
    workbench: &mut NamedSecretWorkbench,
    inputs: &mut SecretInputs,
    config: &mut Config,
) -> bool {
    let mut changed = false;
    if names.is_empty() {
        super::dim_label(ui, "No named secret yet. Add one below, and start an agent panel.");
        return changed;
    }
    let mut delete = None;
    let mut replace = None;
    for (_, name) in names {
        // A name that the rule refuses has no row here.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(name.as_str())
                    .color(theme::FG())
                    .size(12.0)
                    .strong(),
            );
            let (text, color) = state_label(workbench.state_of(name.as_str()));
            ui.label(egui::RichText::new(text).color(color).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Delete").clicked() {
                    delete = Some(name.clone());
                }
                if ui.button("Replace value").clicked() {
                    replace = Some(name.clone());
                }
            });
        });
        if inputs.replacing.as_ref() == Some(name) {
            ui.horizontal(|ui| {
                value_field(ui, &mut inputs.replacement, ("named-secret-replacement", name.as_str()));
                if ui.button("Store").clicked() {
                    let value = SecretInputs::take(&mut inputs.replacement);
                    changed |= store_value(name.clone(), value, workbench, &mut inputs.refusal);
                    inputs.replacing = None;
                }
                if ui.button("Cancel").clicked() {
                    inputs.replacement.zeroize();
                    inputs.replacing = None;
                }
            });
        }
        if inputs.confirming.as_ref() == Some(name) {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Delete {}? The value on this computer goes, and an agent panel that starts after \
                         this reads no {}. An agent panel that is already running keeps it.",
                        name.as_str(),
                        name.as_str()
                    ))
                    .color(theme::PALETTE_YELLOW())
                    .size(11.0),
                );
                if ui.button("Delete").clicked() {
                    workbench.delete(name.clone());
                    config.secrets.names.retain(|text| text != name.as_str());
                    changed = true;
                    delete = Some(name.clone());
                    inputs.confirming = None;
                }
                if ui.button("Cancel").clicked() {
                    inputs.confirming = None;
                }
            });
        }
        ui.add_space(4.0);
    }
    if let Some(name) = replace {
        inputs.replacement.zeroize();
        inputs.replacing = Some(name);
        inputs.confirming = None;
    }
    if let Some(name) = delete
        && !changed
    {
        inputs.confirming = Some(name);
    }
    changed
}

/// The name field and the hidden value field that add one named secret.
fn render_add(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &mut NamedSecretWorkbench,
    inputs: &mut SecretInputs,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut inputs.name)
                .hint_text("NAME")
                .desired_width(180.0)
                .char_limit(horizon_core::named_secrets::MAX_NAME_CHARS)
                .id_salt("named-secret-name"),
        );
        value_field(ui, &mut inputs.value, "named-secret-value");
        let full = config.secrets.names.len() >= MAX_NAMED_SECRETS;
        let ready = !inputs.name.trim().is_empty() && !inputs.value.is_empty() && !full;
        if ui.add_enabled(ready, egui::Button::new("Add")).clicked() {
            match SecretName::parse(inputs.name.trim()) {
                Ok(name) if config.secrets.names.iter().any(|existing| existing == name.as_str()) => {
                    inputs.refusal = Some(format!("{name} is already a named secret of this computer"));
                }
                Ok(_) if full => {
                    inputs.refusal = Some(format!("This computer keeps at most {MAX_NAMED_SECRETS} named secrets"));
                }
                Ok(name) => {
                    let value = SecretInputs::take(&mut inputs.value);
                    if store_value(name.clone(), value, workbench, &mut inputs.refusal) {
                        changed = true;
                        config.secrets.names.push(name.as_str().to_owned());
                        inputs.name.zeroize();
                    }
                }
                Err(error) => {
                    inputs.refusal = Some(error.to_string());
                }
            }
        }
    });
    changed
}

/// The hidden field of one value. The text is never drawn, and the undo history of
/// the field is cleared so a value is not kept in it.
fn value_field(ui: &mut Ui, buffer: &mut Zeroizing<String>, salt: impl egui::AsIdSalt) {
    let mut output = egui::TextEdit::singleline(&mut **buffer)
        .password(true)
        .char_limit(VALUE_CHAR_LIMIT)
        .hint_text("value")
        .desired_width(220.0)
        .id_salt(salt)
        .show(ui);
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
}

/// Checks one typed value and hands it to the worker. Answers whether a check passed;
/// the store answers later, as the state of the row.
fn store_value(
    name: SecretName,
    value: Zeroizing<Vec<u8>>,
    workbench: &mut NamedSecretWorkbench,
    refusal: &mut Option<String>,
) -> bool {
    match check_value(&name, &value) {
        Ok(()) => {
            *refusal = None;
            workbench.store(name, value);
            true
        }
        Err(error) => {
            *refusal = Some(refusal_line(&error, &name));
            false
        }
    }
}

/// The refusal of a value or of a name, in the words of its rule.
fn refusal_line(error: &NamedSecretError, name: &SecretName) -> String {
    match error {
        NamedSecretError::InvalidValue { .. } => {
            format!("The value of {name} is empty or larger than {MAX_VALUE_BYTES} bytes")
        }
        NamedSecretError::ValueNotText { .. } => {
            format!("The value of {name} holds a NUL byte or a line break, or it is not text")
        }
        other => other.to_string(),
    }
}

/// The line and the color of the state of one name on this computer.
fn state_label(state: NamedSecretState) -> (&'static str, egui::Color32) {
    match state {
        NamedSecretState::Present => (NamedSecretState::Present.text(), theme::PALETTE_GREEN()),
        NamedSecretState::NoValue => (NamedSecretState::NoValue.text(), theme::PALETTE_YELLOW()),
        NamedSecretState::Locked | NamedSecretState::StoreUnavailable => (state.text(), theme::PALETTE_RED()),
        NamedSecretState::Checking => (state.text(), theme::FG_DIM()),
    }
}
