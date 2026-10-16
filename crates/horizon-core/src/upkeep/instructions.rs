//! Which instructions an Upkeep's agents get. Saved instructions follow the built-in
//! [`DEFAULT_INSTRUCTIONS`] unless the person changed them: a new default then reaches every
//! Upkeep whose person kept the default, with its next exchange.
//!
//! A saved choice marks with `instructions_edited` whether its text differs from the default
//! of the Horizon that saved it. A choice that an older Horizon saved has no mark: it follows
//! the default when its text is one of the defaults that Horizon has shipped.
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use super::DEFAULT_INSTRUCTIONS;
use super::worker::Config;
use crate::cloud_runtime::{Error, Result};

/// The field of a saved choice that marks instructions the person changed.
const MARK: &str = "instructions_edited";

/// SHA-256 of the trimmed text of every default that Horizon has shipped, the current one
/// included. When [`DEFAULT_INSTRUCTIONS`] changes, add the hash of the new text; never
/// remove one.
const SHIPPED: &[&str] = &[
    "36c6df944657c8d57cf00b1552f3ce0815ef59e50cf8c876dadaea4264a3b49d",
    "075db0a1834c9262670382a792994317a78c01e7ab4d826da4e9f2a5b2579b76",
    "b45c5e9b5d82acf955a62675256b547c2fe5c67d99255d9b5447953f1c24ffd8",
];

/// Whether `text` is not the current default, so the person changed it.
#[must_use]
pub fn edited(text: &str) -> bool {
    text.trim() != DEFAULT_INSTRUCTIONS.trim()
}

/// Whether `text` is a default that Horizon has shipped.
#[must_use]
pub fn shipped(text: &str) -> bool {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(text.trim().as_bytes()) {
        let _ = write!(hex, "{byte:02x}");
    }
    SHIPPED.contains(&hex.as_str())
}

/// The text that saves `config`, with the mark.
/// # Errors
/// A choice that cannot be written as JSON.
pub fn save(config: &Config) -> serde_json::Result<String> {
    let mut value = serde_json::to_value(config)?;
    if let Some(fields) = value.as_object_mut() {
        fields.insert(MARK.into(), edited(&config.instructions).into());
    }
    serde_json::to_string_pretty(&value)
}

/// Reads a saved choice. Instructions that the person did not change become the current
/// default.
/// # Errors
/// Text that is not a saved choice.
pub fn parse(text: &str) -> Result<Config> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| Error::Json)?;
    let mark = value.get(MARK).map(serde_json::Value::as_bool);
    let mut config: Config = serde_json::from_value(value).map_err(|_| Error::Json)?;
    let edited = match mark {
        Some(Some(edited)) => edited,
        Some(None) => return Err(Error::Json),
        None => !shipped(&config.instructions),
    };
    if !edited {
        DEFAULT_INSTRUCTIONS.clone_into(&mut config.instructions);
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(instructions: &str, edited: Option<bool>) -> String {
        let mut value = serde_json::json!({
            "repositories": ["acme/web"], "authors": ["app/dependabot"], "agent": "codex",
            "every_minutes": 60, "instructions": instructions,
        });
        if let Some(edited) = edited {
            value[MARK] = edited.into();
        }
        value.to_string()
    }

    #[test]
    fn the_current_default_is_listed_as_shipped() {
        assert!(
            shipped(DEFAULT_INSTRUCTIONS),
            "add the SHA-256 of the new default's trimmed text to SHIPPED"
        );
        assert!(shipped(&format!("\n{DEFAULT_INSTRUCTIONS}  \n")));
        assert!(!shipped("Merge everything."));
    }

    #[test]
    fn instructions_the_person_kept_follow_the_default() {
        let config = parse(&saved(DEFAULT_INSTRUCTIONS, None)).expect("an older choice");
        assert_eq!(config.instructions, DEFAULT_INSTRUCTIONS);
        let config = parse(&saved("An older default", Some(false))).expect("a marked choice");
        assert_eq!(
            config.instructions, DEFAULT_INSTRUCTIONS,
            "the current default replaces the saved copy"
        );
    }

    #[test]
    fn instructions_the_person_changed_stay() {
        let config = parse(&saved("Only merge patch updates.", None)).expect("an older choice");
        assert_eq!(config.instructions, "Only merge patch updates.");
        let config =
            parse(&saved("Only merge patch updates.", Some(true))).expect("a marked choice");
        assert_eq!(config.instructions, "Only merge patch updates.");
        assert!(edited("Only merge patch updates."));
        assert!(!edited(&format!("{DEFAULT_INSTRUCTIONS}\n")));
        assert!(parse("not json").is_err());
        let mut odd: serde_json::Value = serde_json::from_str(&saved("x", None)).expect("json");
        odd[MARK] = 1.into();
        assert!(parse(&odd.to_string()).is_err(), "a mark that is not true or false");
    }

    #[test]
    fn saving_marks_the_text_against_the_current_default() {
        let mut config = parse(&saved(DEFAULT_INSTRUCTIONS, None)).expect("a choice");
        let text = save(&config).expect("saved");
        assert!(text.contains("\"instructions_edited\": false"), "{text}");
        assert_eq!(parse(&text).expect("read back"), config);
        "Only merge patch updates.".clone_into(&mut config.instructions);
        let text = save(&config).expect("saved");
        assert!(text.contains("\"instructions_edited\": true"), "{text}");
        assert_eq!(parse(&text).expect("read back"), config);
    }
}
