//! The notice that Horizon draws itself when Set up cannot start an agent.
//!
//! The words are Horizon's, not an agent's, because there is no agent to say
//! them: the card names every agent program that Set up looked for, whether the
//! agent integrations of this run are on, and the two things the person can do
//! next. Its buttons are Horizon's own controls, so the card holds no choice and
//! sends no click.

use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, Tone};

/// The title of the card.
pub(super) const TITLE: &str = "Set up needs an agent";
/// What the card says when Horizon found no agent program.
pub(super) const NO_PROGRAM: &str = "An agent in a panel does the work of Set up and shows each step as a card. Horizon found no agent program on this computer.";
/// What the card says when the agent integrations of this run are off.
pub(super) const INTEGRATIONS_OFF: &str = "Agent integrations are off, so the agent has no Horizon skills. Start Horizon without HORIZON_AGENT_INTEGRATIONS=off.";
/// The line under the rows, when the integrations are on.
pub(super) const INSTALL_LINE: &str = "Install one agent program and sign in to it. Then press Set up again. Horizon installs nothing for you and stores no login of the agent.";
/// The line under the rows, when the integrations are off.
pub(super) const SWITCH_LINE: &str = "Start Horizon without that switch, then press Set up again. Horizon installs nothing for you and stores no login of the agent.";
/// The label of the row that says whether the skills are installed.
pub(super) const INTEGRATIONS_ROW: &str = "Agent integrations";
/// The id of the button that looks for an agent program again.
pub(super) const LOOK_AGAIN: &str = "look_again";
/// The id of the button that opens the Presets tab of the settings.
pub(super) const OPEN_PRESETS: &str = "open_presets";

/// One agent program that Set up looked for, and whether this computer has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProgramRow {
    /// The name of the agent, as Horizon shows it elsewhere.
    pub(super) name: String,
    /// The program that Horizon looks for.
    pub(super) program: String,
    /// Whether the program of this computer is an executable file.
    pub(super) found: bool,
}

impl ProgramRow {
    /// What the row says on the right: `found` or `not found`.
    fn answer(&self) -> &'static str {
        if self.found { "found" } else { "not found" }
    }
}

/// The card of the notice, with the agent programs that Horizon looked for and
/// whether the agent integrations of this run are on.
///
/// Every text here is fixed, so the notice passes the rules of a card document
/// like an agent's own document does.
pub(super) fn document(rows: &[ProgramRow], integrations: bool) -> BlockDocument {
    let subtitle = if integrations { NO_PROGRAM } else { INTEGRATIONS_OFF };
    let line = if integrations { INSTALL_LINE } else { SWITCH_LINE };
    let mut pairs: Vec<KvRow> = rows
        .iter()
        .map(|row| KvRow {
            label: row.name.clone(),
            value: row.answer().to_owned(),
            mono: true,
        })
        .collect();
    pairs.push(KvRow {
        label: INTEGRATIONS_ROW.to_owned(),
        value: if integrations { "on" } else { "off" }.to_owned(),
        mono: true,
    });
    BlockDocument {
        components: vec![
            Component::Heading {
                text: TITLE.to_owned(),
                subtitle: Some(subtitle.to_owned()),
                badge: None,
            },
            Component::Kv { rows: pairs },
            Component::Text { text: line.to_owned() },
            Component::Buttons {
                items: vec![
                    Button {
                        id: LOOK_AGAIN.to_owned(),
                        label: "Look again".to_owned(),
                        tone: Tone::Primary,
                    },
                    Button {
                        id: OPEN_PRESETS.to_owned(),
                        label: "Open Settings, Presets".to_owned(),
                        tone: Tone::Neutral,
                    },
                ],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use horizon_browser_control::block::{Component, Tone, validate_document};

    use super::{INSTALL_LINE, INTEGRATIONS_OFF, LOOK_AGAIN, NO_PROGRAM, OPEN_PRESETS, ProgramRow, SWITCH_LINE, TITLE};

    fn rows() -> Vec<ProgramRow> {
        vec![
            ProgramRow {
                name: "Claude".to_owned(),
                program: "claude".to_owned(),
                found: false,
            },
            ProgramRow {
                name: "Codex".to_owned(),
                program: "codex".to_owned(),
                found: false,
            },
        ]
    }

    #[test]
    fn the_notice_names_its_title_the_programs_and_the_integrations() {
        let document = super::document(&rows(), true);
        assert_eq!(validate_document(&document), Ok(()), "the notice is a valid card");

        let Component::Heading { text, subtitle, .. } = &document.components[0] else {
            panic!("the notice starts with its title");
        };
        assert_eq!(text, TITLE);
        assert_eq!(subtitle.as_deref(), Some(NO_PROGRAM));

        let Component::Kv { rows } = &document.components[1] else {
            panic!("the notice lists the programs");
        };
        let listed: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.label.as_str(), row.value.as_str()))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("Claude", "not found"),
                ("Codex", "not found"),
                ("Agent integrations", "on")
            ]
        );
        assert!(rows.iter().all(|row| row.mono), "an answer is a value");
    }

    #[test]
    fn the_notice_says_that_the_skills_are_off_and_never_claims_a_program() {
        let document = super::document(&[], false);
        assert_eq!(validate_document(&document), Ok(()));
        let Component::Heading { subtitle, .. } = &document.components[0] else {
            panic!("a heading");
        };
        assert_eq!(
            subtitle.as_deref(),
            Some(INTEGRATIONS_OFF),
            "the exact sentence of the switch"
        );
        let Component::Text { text } = &document.components[2] else {
            panic!("the line of the notice");
        };
        assert_eq!(text, SWITCH_LINE);
        let Component::Kv { rows } = &document.components[1] else {
            panic!("the row of the integrations");
        };
        assert_eq!(rows.len(), 1, "no program was looked for: {rows:?}");
        assert_eq!(rows[0].value, "off");
    }

    #[test]
    fn the_two_buttons_of_the_notice_are_look_again_and_the_presets_tab() {
        let document = super::document(&rows(), true);
        let Component::Text { text } = &document.components[2] else {
            panic!("the line of the notice");
        };
        assert_eq!(text, INSTALL_LINE);

        let Component::Buttons { items } = &document.components[3] else {
            panic!("the notice ends with its buttons");
        };
        let labels: Vec<(&str, &str, Tone)> = items
            .iter()
            .map(|item| (item.id.as_str(), item.label.as_str(), item.tone))
            .collect();
        assert_eq!(
            labels,
            vec![
                (LOOK_AGAIN, "Look again", Tone::Primary),
                (OPEN_PRESETS, "Open Settings, Presets", Tone::Neutral),
            ]
        );
    }

    #[test]
    fn a_found_program_says_so() {
        let rows = vec![ProgramRow {
            name: "Claude".to_owned(),
            program: "claude".to_owned(),
            found: true,
        }];
        let document = super::document(&rows, true);
        let Component::Kv { rows } = &document.components[1] else {
            panic!("a kv");
        };
        assert_eq!(rows[0].value, "found");
    }
}
