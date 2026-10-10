//! Bounded driver diagnostics exclude command values and provider secrets.
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverDiagnostic {
    pub code: &'static str,
    pub message: String,
}

impl DriverDiagnostic {
    pub(crate) fn new(code: &str, message: &str, private: &[&str]) -> Self {
        let code = match code {
            "unknown command" => "unknown command",
            "unknown method" => "unknown method",
            "unsupported operation" => "unsupported operation",
            "not implemented" => "not implemented",
            "invalid argument" => "invalid argument",
            "invalid session id" => "invalid session id",
            "no such alert" => "no such alert",
            "timeout" => "timeout",
            "unknown error" => "unknown error",
            _ => "driver error",
        };
        let message = if message.len() > 8192 {
            "<driver message exceeded diagnostic limit>".into()
        } else {
            message
                .lines()
                .map(|line| {
                    let line: String = line.chars().filter(|c| !c.is_control()).collect();
                    let lower = line.to_ascii_lowercase();
                    if [
                        "authorization",
                        "accesskey",
                        "access_key",
                        "password",
                        "token",
                        "cookie",
                        "secret",
                        "capabilities",
                        "localkey",
                        "api_key",
                        "bearer ",
                        "basic ",
                        "credential",
                        "/users/",
                        "/home/",
                        "/workspace/",
                    ]
                    .iter()
                    .any(|word| lower.contains(word))
                    {
                        "<redacted sensitive driver line>".to_owned()
                    } else {
                        let mut line = redact_urls(&line);
                        for value in private.iter().filter(|value| !value.is_empty()) {
                            line = line.replace(value, "<redacted>");
                            for fragment in value.lines() {
                                let fragment: String = fragment.chars().filter(|c| !c.is_control()).collect();
                                if !fragment.is_empty() {
                                    line = line.replace(&fragment, "<redacted>");
                                    line = line.replace(&redact_urls(&fragment), "<redacted>");
                                }
                            }
                        }
                        line
                    }
                })
                .collect::<Vec<String>>()
                .join(" ")
                .chars()
                .take(1024)
                .collect()
        };
        Self { code, message }
    }

    pub(crate) fn unsupported(&self) -> bool {
        matches!(
            self.code,
            "unknown command" | "unknown method" | "unsupported operation" | "not implemented"
        ) || self.message.to_ascii_lowercase().contains("unknown mobile command")
    }
}

fn redact_urls(value: &str) -> String {
    let mut redacted = value.to_owned();
    for url in value.split_whitespace().filter(|word| word.contains("://")) {
        redacted = redacted.replace(url, "<redacted URL>");
    }
    redacted
}

impl fmt::Display for DriverDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_keep_the_reason_and_redact_command_and_sensitive_lines() {
        let diagnostic = DriverDiagnostic::new(
            "unknown command",
            "mobile: deepLink is unavailable for native-session\nurl=myapp://debug/state\nAuthorization: Basic private\nXcode 14.3 is required",
            &["native-session", "myapp://debug/state"],
        );
        assert!(diagnostic.unsupported());
        assert!(diagnostic.message.contains("mobile: deepLink is unavailable"));
        assert!(diagnostic.message.contains("Xcode 14.3 is required"));
        for private in ["native-session", "myapp://", "Basic private"] {
            assert!(!diagnostic.to_string().contains(private));
        }
        assert_eq!(
            DriverDiagnostic::new("injected code", "reason", &[]).code,
            "driver error"
        );
        assert!(
            DriverDiagnostic::new("unknown error", &"ø".repeat(2000), &[])
                .message
                .chars()
                .count()
                <= 1024
        );
        assert!(
            !DriverDiagnostic::new("unknown error", &"secret".repeat(2000), &[])
                .message
                .contains("secret")
        );
    }

    #[test]
    fn command_values_cannot_erase_sensitive_markers_before_redaction() {
        for (message, private) in [
            ("Authorization: Basic private-header", "Authorization"),
            ("to\0ken: private-header", "token"),
            ("endpoint https://private-header.example/test?key=value", "://"),
        ] {
            let diagnostic = DriverDiagnostic::new("unknown error", message, &[private]);
            assert!(!diagnostic.message.contains("private-header"), "{diagnostic}");
        }
        let diagnostic = DriverDiagnostic::new("invalid argument", "input alpha   beta rejected", &["alpha   beta"]);
        assert_eq!(diagnostic.message, "input <redacted> rejected");
        let diagnostic = DriverDiagnostic::new(
            "invalid argument",
            "input myapp://test private-value rejected",
            &["myapp://test private-value"],
        );
        assert_eq!(diagnostic.message, "input <redacted> rejected");
    }
}
