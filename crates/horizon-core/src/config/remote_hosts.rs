use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Where the Remote Hosts overlay puts new sessions and how it reaches a
/// host's VNC server through SSH.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct RemoteHostsConfig {
    /// Workspace that receives new SSH and VNC panels unless the overlay
    /// picks another one; created on first use.
    pub default_workspace: String,
    /// Port of the VNC server on the remote host's loopback interface,
    /// reached with `ssh -W` rather than exposed on the network.
    pub vnc_port: u16,
}

impl RemoteHostsConfig {
    pub const DEFAULT_WORKSPACE: &'static str = "Remote Sessions";
    pub const DEFAULT_VNC_PORT: u16 = 5900;

    /// # Errors
    /// Rejects a blank workspace name and port zero.
    pub fn validate(&self) -> Result<()> {
        if self.default_workspace.trim().is_empty() {
            return Err(Error::Config("remote_hosts.default_workspace cannot be empty".into()));
        }
        if self.vnc_port == 0 {
            return Err(Error::Config("remote_hosts.vnc_port must be nonzero".into()));
        }
        Ok(())
    }

    /// The configured workspace name without surrounding whitespace.
    #[must_use]
    pub fn default_workspace_name(&self) -> &str {
        self.default_workspace.trim()
    }

    /// The VNC endpoint as seen from the remote host, in Device panel form.
    #[must_use]
    pub fn vnc_target(&self) -> String {
        format!("127.0.0.1:{}", self.vnc_port)
    }
}

/// Rewrite only `remote_hosts.default_workspace` in config source text,
/// leaving comments, ordering and unknown keys alone. Only the block's own
/// two-space-indented key is touched, never a deeper one, and an inline
/// comment on that key line is kept. Returns `None` when the text cannot be
/// patched safely (a non-block `remote_hosts` value), in which case the
/// caller falls back to serializing the config.
#[must_use]
pub fn patch_default_workspace_source(source: &str, name: &str) -> Option<String> {
    let value = serde_yaml::to_string(&name).ok()?;
    let value = value.trim_end_matches('\n').trim_start_matches("--- ").to_string();
    let lines: Vec<&str> = source.lines().collect();
    if lines
        .iter()
        .any(|line| line.starts_with("remote_hosts:") && !is_block_start(line))
    {
        return None;
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    match lines.iter().position(|line| is_block_start(line)) {
        None => {
            out.extend(lines.iter().map(|line| (*line).to_string()));
            out.push("remote_hosts:".to_string());
            out.push(format!("  default_workspace: {value}"));
        }
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|line| !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#'))
                .map_or(lines.len(), |offset| start + 1 + offset);
            let key_line = (start + 1..end).find(|index| is_direct_key(lines[*index], "default_workspace"));
            for (index, line) in lines.iter().enumerate() {
                if Some(index) == key_line {
                    out.push(format!("  default_workspace: {value}{}", inline_comment(line)));
                } else {
                    out.push((*line).to_string());
                }
                if index == start && key_line.is_none() {
                    out.push(format!("  default_workspace: {value}"));
                }
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    Some(text)
}

fn is_block_start(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed == "remote_hosts:" || trimmed.starts_with("remote_hosts: #")
}

/// A key that belongs to the block itself: exactly two spaces of indentation.
fn is_direct_key(line: &str, key: &str) -> bool {
    line.strip_prefix("  ")
        .is_some_and(|rest| !rest.starts_with(' ') && rest.starts_with(key) && rest[key.len()..].starts_with(':'))
}

/// The trailing ` # comment` of a scalar line, or nothing. Quoted scalars
/// are skipped with YAML's escapes in mind: `\"` inside double quotes and a
/// doubled `''` inside single quotes do not end the quote.
fn inline_comment(line: &str) -> &str {
    let mut in_quote = None;
    let mut chars = line.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        match (character, in_quote) {
            ('"' | '\'', None) => in_quote = Some(character),
            ('\\', Some('"')) => {
                chars.next();
            }
            ('\'', Some('\'')) if chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                chars.next();
            }
            (quote, Some(open)) if quote == open => in_quote = None,
            ('#', None) if index > 0 && line[..index].ends_with(' ') => return line[index - 1..].trim_end(),
            _ => {}
        }
    }
    ""
}

impl Default for RemoteHostsConfig {
    fn default() -> Self {
        Self {
            default_workspace: Self::DEFAULT_WORKSPACE.to_string(),
            vnc_port: Self::DEFAULT_VNC_PORT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteHostsConfig;
    use crate::Config;

    #[test]
    fn defaults_name_the_remote_sessions_workspace_and_the_standard_vnc_port() {
        let config = RemoteHostsConfig::default();
        assert_eq!(config.default_workspace_name(), "Remote Sessions");
        assert_eq!(config.vnc_target(), "127.0.0.1:5900");
        assert!(config.validate().is_ok());
        assert_eq!(Config::default().remote_hosts, config);
    }

    #[test]
    fn missing_section_and_partial_sections_fill_in_defaults() {
        let config = Config::from_yaml("version: 11\n").unwrap();
        assert_eq!(config.remote_hosts, RemoteHostsConfig::default());

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 5901\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Remote Sessions");
        assert_eq!(config.remote_hosts.vnc_target(), "127.0.0.1:5901");

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  Ops  '\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Ops");
    }

    #[test]
    fn blank_workspace_and_port_zero_are_rejected() {
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  '\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.default_workspace"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_port"));
    }

    #[test]
    fn patching_the_default_workspace_keeps_comments_and_unknown_keys() {
        let source = "version: 11 # keep\nremote_hosts:\n  # which workspace\n  default_workspace: Remote Sessions # inline\n  vnc_port: 5901\n  nested:\n    default_workspace: deeper\n  future_key: true\npresets: []\n";
        let patched = super::patch_default_workspace_source(source, "Ops").unwrap();
        assert_eq!(
            patched,
            "version: 11 # keep\nremote_hosts:\n  # which workspace\n  default_workspace: Ops # inline\n  vnc_port: 5901\n  nested:\n    default_workspace: deeper\n  future_key: true\npresets: []\n",
            "the inline comment stays and a deeper key of the same name is untouched"
        );
        assert_eq!(
            Config::from_yaml(&patched)
                .unwrap()
                .remote_hosts
                .default_workspace_name(),
            "Ops"
        );

        let without_key = "remote_hosts:\n  vnc_port: 5901\nworkspaces: []\n";
        assert_eq!(
            super::patch_default_workspace_source(without_key, "Ops: lab").unwrap(),
            "remote_hosts:\n  default_workspace: 'Ops: lab'\n  vnc_port: 5901\nworkspaces: []\n"
        );

        let without_section = "version: 11\nworkspaces: [] # none\n";
        let patched = super::patch_default_workspace_source(without_section, "Ops").unwrap();
        assert_eq!(
            patched,
            "version: 11\nworkspaces: [] # none\nremote_hosts:\n  default_workspace: Ops\n"
        );
        assert_eq!(
            Config::from_yaml(&patched)
                .unwrap()
                .remote_hosts
                .default_workspace_name(),
            "Ops"
        );

        assert_eq!(super::patch_default_workspace_source("remote_hosts: {}\n", "Ops"), None);
        assert_eq!(super::inline_comment("  default_workspace: 'a # b' # note"), " # note");
        assert_eq!(super::inline_comment("  default_workspace: Ops"), "");
        assert_eq!(
            super::inline_comment("  default_workspace: \"say \\\"hi\\\" # not\" # note"),
            " # note",
            "an escaped double quote does not end the scalar"
        );
        assert_eq!(
            super::inline_comment("  default_workspace: 'it''s # not' # note"),
            " # note",
            "a doubled single quote does not end the scalar"
        );
        assert_eq!(super::inline_comment("  default_workspace: \"open # not"), "");
    }

    #[test]
    fn section_round_trips_through_yaml() {
        let mut config = Config::default();
        config.remote_hosts.default_workspace = "Ops".into();
        config.remote_hosts.vnc_port = 5901;
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("remote_hosts:\n  default_workspace: Ops\n  vnc_port: 5901\n"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);
    }
}
