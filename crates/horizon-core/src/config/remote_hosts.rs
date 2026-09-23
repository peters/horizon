use std::collections::BTreeMap;

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
    /// Per-host overrides of `vnc_port`, keyed by the label the overlay
    /// shows (an SSH config alias or Tailscale device name) or by the SSH
    /// host name. A label match wins over a host-name match.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub vnc_ports: BTreeMap<String, u16>,
}

impl RemoteHostsConfig {
    pub const DEFAULT_WORKSPACE: &'static str = "Remote Sessions";
    pub const DEFAULT_VNC_PORT: u16 = 5900;

    /// # Errors
    /// Rejects a blank workspace name, a zero `vnc_port`, and any `vnc_ports`
    /// entry with a blank key or a zero port.
    pub fn validate(&self) -> Result<()> {
        if self.default_workspace.trim().is_empty() {
            return Err(Error::Config("remote_hosts.default_workspace cannot be empty".into()));
        }
        if self.vnc_port == 0 {
            return Err(Error::Config("remote_hosts.vnc_port must be nonzero".into()));
        }
        for (host, port) in &self.vnc_ports {
            if host.trim().is_empty() {
                return Err(Error::Config("remote_hosts.vnc_ports keys cannot be empty".into()));
            }
            if *port == 0 {
                return Err(Error::Config(format!("remote_hosts.vnc_ports.{host} must be nonzero")));
            }
        }
        Ok(())
    }

    /// The configured workspace name, exactly as written: workspace names are
    /// not normalized anywhere else, so a default must match them verbatim.
    #[must_use]
    pub fn default_workspace_name(&self) -> &str {
        &self.default_workspace
    }

    /// The VNC port for a host: an explicit override (typed as `:port` in the
    /// overlay filter) beats the per-host map, which beats `vnc_port`.
    #[must_use]
    pub fn vnc_port_for(&self, label: &str, host: &str, port_override: Option<u16>) -> u16 {
        port_override
            .or_else(|| self.vnc_ports.get(label).copied())
            .or_else(|| self.vnc_ports.get(host).copied())
            .unwrap_or(self.vnc_port)
    }

    /// The VNC endpoint as seen from the remote host, in Device panel form.
    #[must_use]
    pub fn vnc_target(&self, label: &str, host: &str, port_override: Option<u16>) -> String {
        format!("127.0.0.1:{}", self.vnc_port_for(label, host, port_override))
    }
}

/// Rewrite only `remote_hosts.default_workspace` in config source text,
/// leaving comments, ordering and unknown keys alone. Only the block's own
/// key is touched (at whatever child indentation the block uses), never a
/// deeper one, and an inline comment on that key line is kept. Returns
/// `None` when the text cannot be patched safely: a flow-style
/// `remote_hosts` value, or an existing value that continues on further
/// lines (a multiline plain scalar or a block scalar). Callers treat `None`
/// as "leave the file alone".
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
            // The block's own child indentation, whatever the file uses.
            let indent = lines[start + 1..end]
                .iter()
                .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                .map_or("  ", |line| &line[..line.len() - line.trim_start().len()]);
            let key_line = (start + 1..end).find(|index| is_direct_key(lines[*index], indent, "default_workspace"));
            // A tagged, anchored or aliased value (`!!str "…"`, `&a …`, `*a`)
            // has a prefix this scanner does not model; leave such files alone.
            if key_line.is_some_and(|index| value_text(lines[index]).starts_with(['!', '&', '*'])) {
                return None;
            }
            // A value that continues on the next lines cannot be replaced one line at a time.
            if let Some(key_index) = key_line
                && lines[key_index + 1..end]
                    .iter()
                    .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                    .is_some_and(|line| line.len() - line.trim_start().len() > indent.len())
            {
                return None;
            }
            for (index, line) in lines.iter().enumerate() {
                if Some(index) == key_line {
                    out.push(format!("{indent}default_workspace: {value}{}", inline_comment(line)));
                } else {
                    out.push((*line).to_string());
                }
                if index == start && key_line.is_none() {
                    out.push(format!("{indent}default_workspace: {value}"));
                }
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    Some(text)
}

/// `remote_hosts:` alone or followed by a comment after any separation whitespace.
fn is_block_start(line: &str) -> bool {
    line.strip_prefix("remote_hosts:").is_some_and(|rest| {
        let rest = rest.trim_start_matches([' ', '\t']);
        rest.is_empty() || rest.starts_with('#')
    })
}

/// A key that belongs to the block itself: exactly the block's child
/// indentation, the exact key, and the `:` delimiter followed by separation
/// whitespace or the end of the line (so `default_workspace::` is another key).
fn is_direct_key(line: &str, indent: &str, key: &str) -> bool {
    line.strip_prefix(indent)
        .and_then(|rest| (!rest.starts_with(' ')).then_some(rest))
        .and_then(|rest| rest.strip_prefix(key))
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t'))
}

/// The trailing ` # comment` of a `key: value` line, or nothing. Quote
/// semantics apply only when the value itself starts with a quote; a plain
/// scalar such as `Bob's Ops` has no delimiters. Inside a quoted value,
/// `\"` (double quotes) and a doubled `''` (single quotes) do not end it.
/// The value part of a `key: value` line, without leading separation whitespace.
fn value_text(line: &str) -> &str {
    line.find(':')
        .map_or(line, |colon| line[colon + 1..].trim_start_matches([' ', '\t']))
}

fn inline_comment(line: &str) -> &str {
    let value_start = line.len() - value_text(line).len();
    let mut chars = line
        .char_indices()
        .skip_while(|(index, _)| *index < value_start)
        .peekable();
    let mut in_quote = match chars.peek() {
        Some((_, quote @ ('"' | '\''))) => {
            let quote = *quote;
            chars.next();
            Some(quote)
        }
        _ => None,
    };
    while let Some((index, character)) = chars.next() {
        match (character, in_quote) {
            ('\\', Some('"')) => {
                chars.next();
            }
            ('\'', Some('\'')) if chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                chars.next();
            }
            (quote, Some(open)) if quote == open => in_quote = None,
            ('#', None) if index > 0 && line[..index].ends_with([' ', '\t']) => {
                return line[index - 1..].trim_end();
            }
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
            vnc_ports: BTreeMap::new(),
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
        assert_eq!(config.vnc_target("lab", "lab.example", None), "127.0.0.1:5900");
        assert!(config.validate().is_ok());
        assert_eq!(Config::default().remote_hosts, config);
    }

    #[test]
    fn missing_section_and_partial_sections_fill_in_defaults() {
        let config = Config::from_yaml("version: 11\n").unwrap();
        assert_eq!(config.remote_hosts, RemoteHostsConfig::default());

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 5901\n").unwrap();
        assert_eq!(config.remote_hosts.default_workspace_name(), "Remote Sessions");
        assert_eq!(
            config.remote_hosts.vnc_target("lab", "lab.example", None),
            "127.0.0.1:5901"
        );

        let config = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  Ops  '\n").unwrap();
        assert_eq!(
            config.remote_hosts.default_workspace_name(),
            "  Ops  ",
            "names stay exact"
        );
    }

    #[test]
    fn blank_workspace_and_port_zero_are_rejected() {
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  default_workspace: '  '\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.default_workspace"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_port: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_port"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_ports:\n    lab: 0\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_ports.lab"));
        let error = Config::from_yaml("version: 11\nremote_hosts:\n  vnc_ports:\n    ' ': 5901\n").unwrap_err();
        assert!(error.to_string().contains("remote_hosts.vnc_ports keys"));
    }

    #[test]
    fn per_host_ports_resolve_override_then_label_then_host_then_global() {
        let config = Config::from_yaml(
            "version: 11\nremote_hosts:\n  vnc_port: 5901\n  vnc_ports:\n    lab: 5902\n    lab.example: 5903\n    db.example: 5904\n",
        )
        .unwrap();
        let hosts = &config.remote_hosts;
        assert_eq!(
            hosts.vnc_port_for("lab", "lab.example", Some(5999)),
            5999,
            "the typed port wins"
        );
        assert_eq!(
            hosts.vnc_port_for("lab", "lab.example", None),
            5902,
            "the label beats the host name"
        );
        assert_eq!(
            hosts.vnc_port_for("db", "db.example", None),
            5904,
            "the host name is the fallback key"
        );
        assert_eq!(
            hosts.vnc_port_for("Lab", "other", None),
            5901,
            "keys are exact, then the global port"
        );
        assert_eq!(hosts.vnc_target("db", "db.example", None), "127.0.0.1:5904");
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

        let four_spaces = "remote_hosts:\n    vnc_port: 5901 # wide\n    nested:\n        default_workspace: deeper\nworkspaces: []\n";
        let patched = super::patch_default_workspace_source(four_spaces, "Ops").unwrap();
        assert_eq!(
            patched,
            "remote_hosts:\n    default_workspace: Ops\n    vnc_port: 5901 # wide\n    nested:\n        default_workspace: deeper\nworkspaces: []\n",
            "the block's own indentation is detected and kept"
        );
        assert_eq!(
            Config::from_yaml(&patched)
                .unwrap()
                .remote_hosts
                .default_workspace_name(),
            "Ops"
        );
        let four_spaces_key = "remote_hosts:\n    default_workspace: Old # note\n    vnc_port: 5901\n";
        assert_eq!(
            super::patch_default_workspace_source(four_spaces_key, "Ops").unwrap(),
            "remote_hosts:\n    default_workspace: Ops # note\n    vnc_port: 5901\n"
        );
    }

    #[test]
    fn patching_inserts_or_refuses_by_layout_and_scans_comments_precisely() {
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
        assert_eq!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace: Remote\n    Sessions\n  vnc_port: 5900\n",
                "Ops"
            ),
            None,
            "a multiline plain scalar is refused rather than half-replaced"
        );
        assert_eq!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace: |\n    Remote Sessions\n",
                "Ops"
            ),
            None,
            "a block scalar is refused"
        );
        assert!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace: Remote Sessions\n\n  # note\n  vnc_port: 5900\n",
                "Ops"
            )
            .is_some(),
            "a blank line or comment after the key is not a continuation"
        );
        assert_eq!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace: Old\n      # indented note\n  vnc_port: 5900\n",
                "Ops"
            )
            .as_deref(),
            Some("remote_hosts:\n  default_workspace: Ops\n      # indented note\n  vnc_port: 5900\n"),
            "an indented comment is not scalar content"
        );
        assert_eq!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace:: legacy\n  vnc_port: 5900\n",
                "Ops"
            )
            .as_deref(),
            Some("remote_hosts:\n  default_workspace: Ops\n  default_workspace:: legacy\n  vnc_port: 5900\n"),
            "a different key that merely starts the same is left alone"
        );
        assert!(super::is_direct_key(
            "  default_workspace:\tOps",
            "  ",
            "default_workspace"
        ));
        assert!(!super::is_direct_key(
            "  default_workspace_x: Ops",
            "  ",
            "default_workspace"
        ));
    }

    #[test]
    fn block_starts_accept_any_comment_spacing_and_decorated_scalars_are_refused() {
        assert!(super::is_block_start("remote_hosts:  # preferences"));
        assert!(super::is_block_start("remote_hosts:\t# preferences"));
        assert!(!super::is_block_start("remote_hosts: {}"));
        assert_eq!(
            super::patch_default_workspace_source("remote_hosts:  # preferences\n  vnc_port: 5900\n", "Ops").as_deref(),
            Some("remote_hosts:  # preferences\n  default_workspace: Ops\n  vnc_port: 5900\n")
        );
        assert_eq!(
            super::patch_default_workspace_source(
                "remote_hosts:\n  default_workspace: !!str \"Ops # east\" # note\n",
                "Ops"
            ),
            None,
            "a tagged scalar is refused rather than mis-scanned"
        );
        assert_eq!(
            super::patch_default_workspace_source("remote_hosts:\n  default_workspace: &name Ops\n", "Ops"),
            None,
            "an anchored scalar is refused"
        );
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
        assert_eq!(
            super::inline_comment("  default_workspace: Bob's Ops # keep"),
            " # keep",
            "an apostrophe in a plain scalar is not a delimiter"
        );
        assert_eq!(super::inline_comment("  default_workspace: it's#not # yes"), " # yes");
        assert_eq!(
            super::inline_comment("  default_workspace: Ops\t# keep"),
            "\t# keep",
            "a tab is separation whitespace too, and the separator is kept"
        );
    }

    #[test]
    fn section_round_trips_through_yaml() {
        let mut config = Config::default();
        config.remote_hosts.default_workspace = "Ops".into();
        config.remote_hosts.vnc_port = 5901;
        let yaml = config.to_yaml().unwrap();
        assert!(
            yaml.contains("remote_hosts:\n  default_workspace: Ops\n  vnc_port: 5901\n"),
            "an empty per-host map is not written"
        );
        assert!(!yaml.contains("vnc_ports"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);

        config.remote_hosts.vnc_ports.insert("lab".into(), 5902);
        let yaml = config.to_yaml().unwrap();
        assert!(yaml.contains("  vnc_port: 5901\n  vnc_ports:\n    lab: 5902\n"));
        assert_eq!(Config::from_yaml(&yaml).unwrap().remote_hosts, config.remote_hosts);
    }
}
