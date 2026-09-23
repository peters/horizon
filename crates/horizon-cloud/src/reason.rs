//! Short provider explanations taken from untrusted error bodies.
//! Only a top-level JSON string is used; the rest of the body is never echoed.

const FIELDS: [&str; 3] = ["error", "message", "detail"];
const MAX_CHARS: usize = 160;

/// Sanitized explanation from a failed provider response; empty when the body offers none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reason(Option<String>);

impl Reason {
    /// No reason is kept when the text repeats `secret` (printable ASCII), however
    /// its characters are separated, and the check precedes truncation so a cut
    /// cannot leave part of it visible.
    pub(crate) fn from_body(body: &[u8], secret: &str) -> Self {
        let Ok(serde_json::Value::Object(fields)) = serde_json::from_slice(body) else {
            return Self::default();
        };
        let Some(raw) = FIELDS.iter().find_map(|name| fields.get(*name)?.as_str()) else {
            return Self::default();
        };
        let mut remaining = raw.chars();
        if !secret.is_empty() && secret.chars().all(|expected| remaining.any(|c| c == expected)) {
            return Self::default();
        }
        let text = collapse(raw);
        if text.is_empty() {
            return Self::default();
        }
        Self(Some(truncate(text)))
    }

    /// `": explanation"`, or nothing, for appending to an error message.
    pub(crate) fn suffix(&self) -> String {
        self.0.as_ref().map_or_else(String::new, |text| format!(": {text}"))
    }
}

fn collapse(raw: &str) -> String {
    let visible: String = raw
        .chars()
        .filter(|&c| !default_ignorable(c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    visible.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Unicode 16's `Default_Ignorable_Code_Point` set, which includes every bidirectional
/// formatting control; such characters can disguise the displayed text.
fn default_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "secret-test-key";

    fn reason(body: &str) -> Option<String> {
        Reason::from_body(body.as_bytes(), SECRET).0
    }

    #[test]
    fn takes_the_first_known_top_level_string_field() {
        assert_eq!(
            reason(r#"{"error":"create pod: no instances available","status":500}"#).as_deref(),
            Some("create pod: no instances available")
        );
        assert_eq!(
            reason(r#"{"detail":"Request validation failed.","errors":["x"]}"#).as_deref(),
            Some("Request validation failed.")
        );
        assert_eq!(
            reason(r#"{"message":"quota","detail":"ignored"}"#).as_deref(),
            Some("quota")
        );
        assert_eq!(reason(r#"{"error":{"message":"nested"}}"#), None);
    }

    #[test]
    fn non_json_or_missing_fields_give_no_reason() {
        for body in [
            "",
            "refused",
            "<html>502 Bad Gateway</html>",
            r#"["error"]"#,
            r#"{"status":500}"#,
        ] {
            assert_eq!(reason(body), None, "{body}");
        }
        assert_eq!(reason(r#"{"error":"  \n\t "}"#), None);
        assert_eq!(Reason::default().suffix(), "");
    }

    #[test]
    fn control_and_formatting_characters_are_removed_and_whitespace_collapsed() {
        assert_eq!(
            reason("{\"error\":\"line one\\nline\\u0000two\\u001b[31m  red\\u202e\\u200b\\u061c end\\u00ad\\u034f\\u180e\\u206a\\ufe0f\\udb40\\udc41\"}").as_deref(),
            Some("line one line two [31m red end")
        );
    }

    #[test]
    fn long_text_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(400);
        let text = reason(&format!(r#"{{"error":"{long}"}}"#)).unwrap();
        assert_eq!(text.chars().count(), MAX_CHARS);
        assert!(text.ends_with('…'));
        let exact = "a".repeat(MAX_CHARS);
        assert_eq!(reason(&format!(r#"{{"error":"{exact}"}}"#)), Some(exact));
        let spaced = format!("{} {}", "a".repeat(MAX_CHARS - 2), "b".repeat(10));
        assert_eq!(
            reason(&format!(r#"{{"error":"{spaced}"}}"#)),
            Some(format!("{}…", "a".repeat(MAX_CHARS - 2)))
        );
    }

    #[test]
    fn a_reason_repeating_the_secret_is_dropped_even_where_display_would_cut_or_hide_it() {
        assert_eq!(reason(&format!(r#"{{"error":"bad key {SECRET}"}}"#)), None);
        let straddling = format!("{}{SECRET}", "a".repeat(MAX_CHARS - 5));
        assert_eq!(reason(&format!(r#"{{"error":"{straddling}"}}"#)), None);
        for separator in [
            "\\u200b",
            "\\u206a",
            "\\u00ad",
            "\\udb40\\udc41",
            " ",
            "\\n",
            "é",
            ":",
            "s-t",
            "XYZ",
        ] {
            let body = format!(r#"{{"error":"bad key secret-{separator}test-key"}}"#);
            assert_eq!(reason(&body), None, "{body}");
        }
        assert_eq!(
            Reason::from_body(br#"{"error":"bad key secret-test-key"}"#, "other").suffix(),
            ": bad key secret-test-key"
        );
    }
}
