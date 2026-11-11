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

/// The size of the name of one secret, and of its state line.
const NAME_SIZE: f32 = 12.0;
const STATE_SIZE: f32 = 11.0;

/// The share of the name line that the name may take, so the state keeps a
/// readable part of the line beside a long name.
const NAME_SHARE: f32 = 0.55;

/// The share of the fields that the name field of the add row takes, so the
/// value field keeps the larger part of the row.
const NAME_FIELD_SHARE: f32 = 0.45;

/// The narrowest field of the row that adds one secret, in points. The two
/// fields share what the button of that row leaves, and this keeps a field that
/// a person can still type into at the narrowest panel.
const MIN_FIELD_WIDTH: f32 = 80.0;

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

    /// The rows of one name that a person opened, for a test of their layout.
    #[cfg(test)]
    pub(super) fn with_open_rows(name: &SecretName) -> Self {
        Self {
            replacing: Some(name.clone()),
            confirming: Some(name.clone()),
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
                egui::RichText::new(format!("The secrets file of this computer: {error}"))
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
        // A name that the rule refuses has no row here. The name and its state
        // share one line and are cut there, and the two actions take the line
        // under them, so a long name, a long state or the narrowest panel never
        // widens the row past the panel beside the canvas.
        ui.horizontal(|ui| {
            let name_width = (ui.available_width() * NAME_SHARE).max(0.0);
            ui.scope(|ui| {
                ui.set_max_width(name_width);
                cut_label(ui, name.as_str(), NAME_SIZE, theme::FG(), true);
            });
            let (text, color) = state_label(workbench.state_of(name.as_str()));
            cut_label(ui, text, STATE_SIZE, color, false);
        });
        ui.horizontal_wrapped(|ui| {
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
            changed |= render_replacement(ui, name, workbench, inputs);
        }
        if inputs.confirming.as_ref() == Some(name) && render_delete_question(ui, name, inputs) {
            workbench.delete(name.clone());
            config.secrets.names.retain(|text| text != name.as_str());
            changed = true;
            delete = Some(name.clone());
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

/// The row that takes a new value for `name`. Answers whether the list changed.
fn render_replacement(
    ui: &mut Ui,
    name: &SecretName,
    workbench: &mut NamedSecretWorkbench,
    inputs: &mut SecretInputs,
) -> bool {
    let mut store = false;
    let mut cancel = false;
    super::action_row(
        ui,
        |ui| {
            value_field(
                ui,
                &mut inputs.replacement,
                ("named-secret-replacement", name.as_str()),
                ui.available_width(),
            );
        },
        |ui| {
            store = ui.button("Store").clicked();
            cancel = ui.button("Cancel").clicked();
        },
    );
    if store {
        let value = SecretInputs::take(&mut inputs.replacement);
        let changed = store_value(name.clone(), value, workbench, &mut inputs.refusal);
        inputs.replacing = None;
        return changed;
    }
    if cancel {
        inputs.replacement.zeroize();
        inputs.replacing = None;
    }
    false
}

/// The question that confirms that the value of `name` goes. It names the secret
/// twice, so it wraps inside the panel and its two actions take the line under
/// it. Answers whether the person confirmed the delete.
fn render_delete_question(ui: &mut Ui, name: &SecretName, inputs: &mut SecretInputs) -> bool {
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "Delete {}? The value on this computer goes, and an agent panel that starts after \
                 this reads no {}. An agent panel that is already running keeps it.",
                name.as_str(),
                name.as_str()
            ))
            .color(theme::PALETTE_YELLOW())
            .size(STATE_SIZE),
        )
        .wrap(),
    );
    let mut confirmed = false;
    let mut cancelled = false;
    ui.horizontal_wrapped(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            confirmed = ui.button("Delete").clicked();
            cancelled = ui.button("Cancel").clicked();
        });
    });
    if confirmed || cancelled {
        inputs.confirming = None;
    }
    confirmed
}

/// The name field and the hidden value field that add one named secret.
fn render_add(
    ui: &mut Ui,
    config: &mut Config,
    workbench: &mut NamedSecretWorkbench,
    inputs: &mut SecretInputs,
) -> bool {
    let full = config.secrets.names.len() >= MAX_NAMED_SECRETS;
    let ready = !inputs.name.trim().is_empty() && !inputs.value.is_empty() && !full;
    // The button is taken from the right first, so the two fields share what it
    // leaves: a field that kept its wanted width would push the button past the
    // panel and widen the row.
    let mut added = false;
    super::action_row(
        ui,
        |ui| {
            let gap = ui.spacing().item_spacing.x;
            let fields = (ui.available_width() - gap).max(0.0);
            let name_width = (fields * NAME_FIELD_SHARE).max(MIN_FIELD_WIDTH);
            ui.add(
                egui::TextEdit::singleline(&mut inputs.name)
                    .hint_text("NAME")
                    .desired_width(name_width)
                    .char_limit(horizon_core::named_secrets::MAX_NAME_CHARS)
                    .id_salt("named-secret-name"),
            );
            value_field(ui, &mut inputs.value, "named-secret-value", ui.available_width());
        },
        |ui| {
            added = ui.add_enabled(ready, egui::Button::new("Add")).clicked();
        },
    );
    if added {
        add_secret(config, workbench, inputs)
    } else {
        false
    }
}

/// Adds the name and the value that the person typed, and answers whether the
/// list changed.
fn add_secret(config: &mut Config, workbench: &mut NamedSecretWorkbench, inputs: &mut SecretInputs) -> bool {
    let full = config.secrets.names.len() >= MAX_NAMED_SECRETS;
    match SecretName::parse(inputs.name.trim()) {
        Ok(name) if config.secrets.names.iter().any(|existing| existing == name.as_str()) => {
            inputs.refusal = Some(format!("{name} is already a named secret of this computer"));
            false
        }
        Ok(_) if full => {
            inputs.refusal = Some(format!("This computer keeps at most {MAX_NAMED_SECRETS} named secrets"));
            false
        }
        Ok(name) => {
            let value = SecretInputs::take(&mut inputs.value);
            if store_value(name.clone(), value, workbench, &mut inputs.refusal) {
                config.secrets.names.push(name.as_str().to_owned());
                inputs.name.zeroize();
                true
            } else {
                false
            }
        }
        Err(error) => {
            inputs.refusal = Some(error.to_string());
            false
        }
    }
}

/// The hidden field of one value, as wide as `width`. The text is never drawn,
/// and the undo history of the field is cleared so a value is not kept in it.
fn value_field(ui: &mut Ui, buffer: &mut Zeroizing<String>, salt: impl egui::AsIdSalt, width: f32) {
    let mut output = egui::TextEdit::singleline(&mut **buffer)
        .password(true)
        .char_limit(VALUE_CHAR_LIMIT)
        .hint_text("value")
        .desired_width(width.max(MIN_FIELD_WIDTH))
        .id_salt(salt)
        .show(ui);
    output.state.clear_undoer();
    output.state.store(ui.ctx(), output.response.id);
}

/// One single-line text of a row, cut at the width that its place gives it. A
/// text that this cut short shows itself whole in its hover text, so the person
/// reads the whole name or the whole state.
fn cut_label(ui: &mut Ui, text: &str, size: f32, color: egui::Color32, strong: bool) -> egui::Response {
    let mut rich = egui::RichText::new(text).color(color).size(size);
    if strong {
        rich = rich.strong();
    }
    let whole = egui::WidgetText::from(rich.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let response = ui.add(egui::Label::new(rich).truncate());
    if response.rect.width() + 1.0 < whole.size().x {
        return response.on_hover_text(text.to_owned());
    }
    response
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
        NamedSecretState::Unavailable => (state.text(), theme::PALETTE_RED()),
        NamedSecretState::Checking => (state.text(), theme::FG_DIM()),
    }
}
