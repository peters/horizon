//! The `secrets` section of the settings: the names of the named secrets of a
//! person, and never a value.
//!
//! A value lives in the OS credential store, so this section is the list that the
//! Settings window draws and the list that the panels of this computer read. A name
//! whose value this computer does not hold yet is still a name here: the server of
//! the person brings the value, and a computer that has not received it yet shows
//! that it holds none.

use serde::{Deserialize, Serialize};

use crate::named_secrets::{MAX_NAMED_SECRETS, SecretName};

/// The named secrets of this computer, by name.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SecretsConfig {
    /// The names of the named secrets, in the order that the person keeps them.
    pub names: Vec<String>,
}

impl SecretsConfig {
    /// Why this section cannot be used, in the words of its rule.
    ///
    /// # Errors
    /// A name that is not one name of a named secret, the same name twice, and more
    /// names than this Horizon keeps.
    pub fn validate(&self) -> Result<(), String> {
        if self.names.len() > MAX_NAMED_SECRETS {
            return Err(format!(
                "secrets.names holds {} names, and Horizon keeps {MAX_NAMED_SECRETS}",
                self.names.len()
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.names {
            let parsed = SecretName::parse(name).map_err(|error| format!("secrets.names: {error}"))?;
            if !seen.insert(parsed.clone()) {
                return Err(format!("secrets.names holds {parsed} twice"));
            }
        }
        Ok(())
    }

    /// The names of this section that pass the rule, in name order, for the code
    /// that reads the credential store. A name that the rule refuses is dropped, so
    /// one bad name does not stop the others.
    #[must_use]
    pub fn secret_names(&self) -> Vec<SecretName> {
        let mut names: Vec<SecretName> = self
            .names
            .iter()
            .filter_map(|name| SecretName::parse(name).ok())
            .collect();
        names.sort();
        names.dedup();
        names.truncate(MAX_NAMED_SECRETS);
        names
    }
}

#[cfg(test)]
mod tests {
    use super::SecretsConfig;

    #[test]
    fn a_section_of_names_round_trips_and_holds_no_value() {
        let section = SecretsConfig {
            names: vec!["HETZNER_TOKEN".to_owned(), "CLOUD_KEY".to_owned()],
        };
        let document = serde_json::to_string(&section).expect("the section");
        assert_eq!(document, r#"{"names":["HETZNER_TOKEN","CLOUD_KEY"]}"#);
        let parsed: SecretsConfig = serde_json::from_str(&document).expect("the section");
        assert_eq!(parsed, section);
        assert!(parsed.validate().is_ok());
        assert_eq!(parsed.secret_names().len(), 2);
    }

    #[test]
    fn a_name_that_is_not_one_name_and_a_reserved_name_are_refused() {
        for refused in ["lower", "HOME", "HORIZON_HOME", "with-dash", ""] {
            let section = SecretsConfig {
                names: vec![refused.to_owned()],
            };
            let error = section.validate().expect_err("a refused name");
            assert!(error.contains("secrets.names"), "{error}");
        }
        let twice = SecretsConfig {
            names: vec!["KEY".to_owned(), "KEY".to_owned()],
        };
        assert!(twice.validate().expect_err("a duplicate").contains("twice"));
    }

    #[test]
    fn a_settings_document_that_names_secrets_parses_and_validates() {
        // The document that the smoke fixture writes, so its first start imports the
        // names and no value.
        let document = r#"{"version": 11, "secrets": {"names": ["HETZNER_TOKEN"]}}"#;
        let config = crate::Config::from_json(document).expect("the document of the fixture");
        assert_eq!(config.secrets.names, vec!["HETZNER_TOKEN".to_owned()]);
        assert_eq!(config.secrets.secret_names().len(), 1);

        let refused = r#"{"version": 11, "secrets": {"names": ["HOME"]}}"#;
        assert!(
            crate::Config::from_json(refused).is_err(),
            "a reserved name is refused before a start reads it"
        );
    }

    #[test]
    fn the_section_holds_at_most_sixty_four_names() {
        let names = |count: usize| SecretsConfig {
            names: (0..count).map(|index| format!("KEY_{index}")).collect(),
        };
        assert!(names(64).validate().is_ok(), "64 names are a section");
        assert!(names(65).validate().is_err(), "65 names are not");
        assert_eq!(names(70).secret_names().len(), 64);
    }
}
