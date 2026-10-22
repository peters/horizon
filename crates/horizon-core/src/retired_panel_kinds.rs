//! Panel kinds that a later release removed.
//!
//! A config, a preset or a saved board can still name one of them. The typed form no longer
//! has the variant, so a plain parse refuses the whole file. The loaders drop these entries
//! first, and the person keeps the other workspaces, panels, presets and settings.

use std::borrow::Cow;

use serde_yaml::Value;

/// Panel kinds that a later release removed, as they appear in YAML.
const RETIRED: [&str; 1] = ["upkeep"];

const KIND: &str = "kind";

/// `content` with every entry that names a retired panel kind removed.
///
/// `content` that this walk does not understand comes back unchanged, so the typed parse
/// reports the problem in its own terms.
pub(crate) fn without_retired(content: &str) -> Cow<'_, str> {
    let Ok(mut value) = serde_yaml::from_str::<Value>(content) else {
        return Cow::Borrowed(content);
    };
    if !drop_entries(&mut value) {
        return Cow::Borrowed(content);
    }
    match serde_yaml::to_string(&value) {
        Ok(scrubbed) => Cow::Owned(scrubbed),
        Err(_) => Cow::Borrowed(content),
    }
}

/// Remove the retired entries from every sequence in `value`. Returns whether it removed one.
fn drop_entries(value: &mut Value) -> bool {
    match value {
        Value::Mapping(mapping) => {
            let mut removed = false;
            for (_, entry) in mapping.iter_mut() {
                removed |= drop_entries(entry);
            }
            removed
        }
        Value::Sequence(items) => {
            let before = items.len();
            items.retain(|item| !is_retired(item));
            let mut removed = items.len() != before;
            for item in items.iter_mut() {
                removed |= drop_entries(item);
            }
            removed
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Tagged(_) => false,
    }
}

fn is_retired(item: &Value) -> bool {
    item.get(KIND)
        .and_then(Value::as_str)
        .is_some_and(|kind| RETIRED.contains(&kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upkeep_terminal_goes_and_the_others_stay() {
        let content = "\
workspaces:
- name: Upkeep
  terminals:
  - name: Upkeep
    kind: upkeep
- name: Work
  terminals:
  - name: Shell
    kind: shell
";
        let scrubbed = without_retired(content);
        assert!(!scrubbed.contains("upkeep"), "{scrubbed}");
        assert!(scrubbed.contains("name: Work"), "{scrubbed}");
        assert!(scrubbed.contains("name: Shell"), "{scrubbed}");
    }

    #[test]
    fn a_preset_and_a_panel_of_the_retired_kind_go_too() {
        let content = "\
presets:
- name: Upkeep
  kind: upkeep
- name: Shell
  kind: shell
workspaces:
- name: Work
  panels:
  - kind: upkeep
  - kind: shell
";
        let scrubbed = without_retired(content);
        assert!(!scrubbed.contains("upkeep"), "{scrubbed}");
        assert!(scrubbed.contains("name: Shell"), "{scrubbed}");
        assert!(scrubbed.contains("kind: shell"), "{scrubbed}");
    }

    #[test]
    fn content_without_a_retired_kind_comes_back_untouched() {
        let content = "workspaces:\n- name: Work\n  terminals:\n  - name: Shell\n    kind: shell\n";
        assert!(matches!(without_retired(content), Cow::Borrowed(_)), "no rewrite");
    }

    #[test]
    fn content_that_is_not_yaml_comes_back_untouched() {
        let content = "workspaces: [unclosed\n";
        assert!(matches!(without_retired(content), Cow::Borrowed(_)), "no rewrite");
    }
}
