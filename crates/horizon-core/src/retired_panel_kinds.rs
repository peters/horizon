//! Panel kinds that a later release removed.
//!
//! A config, a preset or a saved board can still name one of them. The typed form no longer
//! has the variant, so a plain parse refuses the whole file. The loaders drop these entries
//! first, and the person keeps the other workspaces, panels, presets and settings.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde_yaml::Value;

/// Panel kinds that a later release removed, as they appear in a document.
const RETIRED: [&str; 3] = ["upkeep", "git_changes", "usage"];

const KIND: &str = "kind";

/// The entries of a document that named a removed panel kind, counted by that name.
///
/// A board that a person saved with an older release keeps the panels of the
/// removed kinds out, and this count is what a notice to the person is made of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetiredPanels {
    counts: BTreeMap<&'static str, usize>,
}

impl RetiredPanels {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The number of entries that were left out.
    #[must_use]
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }

    /// The number of entries that named `kind`.
    #[must_use]
    pub fn count_of(&self, kind: &str) -> usize {
        self.counts.get(kind).copied().unwrap_or(0)
    }

    /// The counts, by name.
    #[must_use]
    pub fn kinds(&self) -> Vec<(&'static str, usize)> {
        self.counts.iter().map(|(kind, count)| (*kind, *count)).collect()
    }

    pub(crate) fn count(&mut self, kind: &'static str) {
        *self.counts.entry(kind).or_default() += 1;
    }
}

impl fmt::Display for RetiredPanels {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (kind, count)) in self.counts.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{kind}={count}")?;
        }
        Ok(())
    }
}

/// `content` with every entry that names a retired panel kind removed.
///
/// The settings of a computer are JSON and a saved board is still YAML, so the
/// walk reads YAML, which also reads JSON, and answers JSON, which both readers
/// accept.
///
/// `content` that this walk does not understand comes back unchanged, so the typed parse
/// reports the problem in its own terms.
pub(crate) fn without_retired(content: &str) -> Cow<'_, str> {
    without_retired_reporting(content).0
}

/// [`without_retired`], and the entries that it left out.
pub(crate) fn without_retired_reporting(content: &str) -> (Cow<'_, str>, RetiredPanels) {
    let mut left_out = RetiredPanels::default();
    let Ok(mut value) = serde_yaml::from_str::<Value>(content) else {
        return (Cow::Borrowed(content), left_out);
    };
    if !drop_entries(&mut value, &mut left_out) {
        return (Cow::Borrowed(content), left_out);
    }
    match serde_json::to_string(&value) {
        Ok(scrubbed) => (Cow::Owned(scrubbed), left_out),
        Err(_) => (Cow::Borrowed(content), RetiredPanels::default()),
    }
}

/// The retired kind that one stored entry names, when it names one.
///
/// A board is also kept as one row for each panel, and this reads such a row.
pub(crate) fn retired_kind_named(text: &str) -> Option<&'static str> {
    serde_yaml::from_str::<Value>(text)
        .ok()
        .and_then(|value| retired_kind(&value))
}

/// Remove the retired entries from every sequence in `value`. Returns whether it removed one.
fn drop_entries(value: &mut Value, left_out: &mut RetiredPanels) -> bool {
    match value {
        Value::Mapping(mapping) => {
            let mut removed = false;
            for entry in mapping.values_mut() {
                removed |= drop_entries(entry, left_out);
            }
            removed
        }
        Value::Sequence(items) => {
            let mut removed = false;
            let mut kept = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                if let Some(kind) = retired_kind(&item) {
                    left_out.count(kind);
                    removed = true;
                    continue;
                }
                removed |= drop_entries(&mut item, left_out);
                kept.push(item);
            }
            *items = kept;
            removed
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Tagged(_) => false,
    }
}

fn retired_kind(item: &Value) -> Option<&'static str> {
    item.get(KIND)
        .and_then(Value::as_str)
        .and_then(|kind| RETIRED.into_iter().find(|retired| *retired == kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upkeep_terminal_goes_and_the_others_stay() {
        let content = r#"{
  "workspaces": [
    {"name": "Upkeep", "terminals": [{"name": "Upkeep", "kind": "upkeep"}]},
    {"name": "Work", "terminals": [{"name": "Shell", "kind": "shell"}]}
  ]
}"#;
        let scrubbed = without_retired(content);
        assert!(!scrubbed.contains("upkeep"), "{scrubbed}");
        assert!(scrubbed.contains("\"Work\""), "{scrubbed}");
        assert!(scrubbed.contains("\"Shell\""), "{scrubbed}");
    }

    #[test]
    fn a_saved_board_in_yaml_is_scrubbed_too() {
        let content = "\
workspaces:
- name: Work
  panels:
  - name: Upkeep
    kind: upkeep
  - name: Shell
    kind: shell
";
        let scrubbed = without_retired(content);
        assert!(!scrubbed.contains("upkeep"), "{scrubbed}");
        assert!(scrubbed.contains("\"Shell\""), "the answer is JSON: {scrubbed}");
    }

    #[test]
    fn a_preset_and_a_panel_of_the_retired_kind_go_too() {
        let content = r#"{
  "presets": [{"name": "Upkeep", "kind": "upkeep"}, {"name": "Shell", "kind": "shell"}],
  "workspaces": [{"name": "Work", "panels": [{"kind": "upkeep"}, {"kind": "shell"}]}]
}"#;
        let scrubbed = without_retired(content);
        assert!(!scrubbed.contains("upkeep"), "{scrubbed}");
        assert!(scrubbed.contains("\"name\":\"Shell\""), "{scrubbed}");
        assert!(scrubbed.contains("\"kind\":\"shell\""), "{scrubbed}");
    }

    #[test]
    fn content_without_a_retired_kind_comes_back_untouched() {
        let content = r#"{"workspaces": [{"name": "Work", "terminals": [{"name": "Shell", "kind": "shell"}]}]}"#;
        assert!(matches!(without_retired(content), Cow::Borrowed(_)), "no rewrite");
    }

    #[test]
    fn content_that_parses_as_neither_format_comes_back_untouched() {
        let content = "{\"workspaces\": [unclosed\n";
        assert!(matches!(without_retired(content), Cow::Borrowed(_)), "no rewrite");
    }

    #[test]
    fn a_panel_of_a_removed_kind_is_counted_by_its_name() {
        let content = "\
workspaces:
- name: Work
  panels:
  - name: Usage
    kind: usage
  - name: Git Changes
    kind: git_changes
  - name: Another Usage
    kind: usage
  - name: Shell
    kind: shell
";
        let (scrubbed, left_out) = without_retired_reporting(content);

        assert!(!scrubbed.contains("\"usage\""), "{scrubbed}");
        assert!(!scrubbed.contains("\"git_changes\""), "{scrubbed}");
        assert_eq!(left_out.total(), 3);
        assert_eq!(left_out.count_of("usage"), 2);
        assert_eq!(left_out.count_of("git_changes"), 1);
        assert_eq!(left_out.count_of("shell"), 0);
        assert_eq!(left_out.kinds(), vec![("git_changes", 1), ("usage", 2)]);
        assert_eq!(left_out.to_string(), "git_changes=1, usage=2");
    }

    #[test]
    fn a_board_without_a_removed_kind_counts_nothing() {
        let content = "\
workspaces:
- name: Work
  panels:
  - name: Shell
    kind: shell
";
        let (scrubbed, left_out) = without_retired_reporting(content);

        assert!(matches!(scrubbed, Cow::Borrowed(_)), "no rewrite");
        assert!(left_out.is_empty());
        assert_eq!(left_out.to_string(), "");
    }

    #[test]
    fn a_stored_row_names_its_removed_kind() {
        assert_eq!(
            retired_kind_named(r#"{"name": "Usage", "kind": "usage"}"#),
            Some("usage")
        );
        assert_eq!(
            retired_kind_named("name: Git Changes\nkind: git_changes\n"),
            Some("git_changes")
        );
        assert_eq!(retired_kind_named(r#"{"name": "Shell", "kind": "shell"}"#), None);
        assert_eq!(retired_kind_named("not a value"), None);
    }
}
