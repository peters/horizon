//! The owner's scope editor on the cloud card: which devices, and which of this computer's
//! loopback ports, a bridge reaches. Like the switch, it lives in memory only: nothing reads it
//! from configuration or from an agent, and switching sharing off forgets it.
use horizon_core::cloud_runtime::local_network::{Device, Rules};
use std::net::Ipv4Addr;

/// Characters either text field takes: room for the largest scope the rules allow, 32 devices
/// with 16 ports each (about 3,600 characters), with space to spare.
const TEXT_LIMIT: usize = 8192;

#[derive(Default)]
pub(in super::super) struct Editor {
    /// The rules the bridge applies. They outlast a pause, so a resumed bridge starts with them.
    pub(super) applied: Rules,
    devices: String,
    local_ports: String,
    error: Option<String>,
}

impl Editor {
    /// Starts again from the whole network, as a newly switched-on bridge does.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Draws the editor and returns the rules to apply when the owner pressed Apply and the
    /// text reads as rules; [`Self::outcome`] records whether the bridge took them.
    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> Option<Rules> {
        let mut apply = None;
        egui::CollapsingHeader::new(summary(&self.applied))
            .id_salt("local-network-scope")
            .default_open(false)
            .show(ui, |ui| {
                ui.small("Devices, one per line: an address, and optionally its ports after a colon. Empty: every device on the network.");
                ui.add(
                    egui::TextEdit::multiline(&mut self.devices)
                        .desired_rows(3)
                        .char_limit(TEXT_LIMIT)
                        .hint_text("192.168.1.50\n192.168.1.60:22,80"),
                );
                ui.small("This computer's own ports, reached as localhost. Empty: none. While sharing is on, every process on the worker can use them.");
                ui.add(
                    egui::TextEdit::singleline(&mut self.local_ports)
                        .char_limit(TEXT_LIMIT)
                        .hint_text("3000, 8080"),
                );
                if ui.button("Apply scope").clicked() {
                    match parse(&self.devices, &self.local_ports) {
                        Ok(rules) => apply = Some(rules),
                        Err(error) => self.error = Some(error),
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        apply
    }

    /// Keeps `rules` once the bridge applied them, or shows why it refused them and keeps the
    /// rules it had.
    pub(super) fn outcome(&mut self, rules: Rules, applied: Result<(), String>) {
        match applied {
            Ok(()) => {
                self.applied = rules;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

/// The collapsed header: what the bridge reaches now.
fn summary(rules: &Rules) -> String {
    let devices = match rules.devices.len() {
        0 => "the whole network".to_owned(),
        1 => "1 device".to_owned(),
        count => format!("{count} devices"),
    };
    match rules.local_ports.len() {
        0 => format!("Scope: {devices}"),
        1 => format!("Scope: {devices} · 1 port on this computer"),
        count => format!("Scope: {devices} · {count} ports on this computer"),
    }
}

/// Reads the two fields as rules; the bridge checks them against the network when applied.
///
/// # Errors
/// Names the line or field that does not read as an address or a port.
fn parse(devices: &str, local_ports: &str) -> Result<Rules, String> {
    let mut rules = Rules::default();
    for (index, line) in devices.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (address, ports) = match line.split_once(':') {
            Some((address, ports)) => (address.trim(), Some(ports)),
            None => (line, None),
        };
        let number = index + 1;
        let address: Ipv4Addr = address
            .parse()
            .map_err(|_| format!("Line {number}: {address} is not an IPv4 address"))?;
        let ports = match ports {
            None => Vec::new(),
            Some(ports) => match self::ports(ports) {
                Ok(ports) if ports.is_empty() => return Err(format!("Line {number}: no ports after the colon")),
                Ok(ports) => ports,
                Err(error) => return Err(format!("Line {number}: {error}")),
            },
        };
        rules.devices.push(Device { address, ports });
    }
    rules.local_ports = ports(local_ports).map_err(|error| format!("This computer: {error}"))?;
    Ok(rules)
}

/// Ports separated by commas or spaces.
fn ports(text: &str) -> Result<Vec<u16>, String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|port| !port.is_empty())
        .map(|port| {
            port.parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| format!("{port} is not a port"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(address: [u8; 4], ports: &[u16]) -> Device {
        Device {
            address: Ipv4Addr::from(address),
            ports: ports.to_vec(),
        }
    }

    #[test]
    fn the_fields_read_as_devices_with_optional_ports_and_this_computers_ports() {
        assert_eq!(parse("", ""), Ok(Rules::default()), "empty is the whole network");
        assert_eq!(
            parse(" 192.168.1.50 \n\n192.168.1.60: 22, 80\n", "3000 8080,9000"),
            Ok(Rules {
                devices: vec![device([192, 168, 1, 50], &[]), device([192, 168, 1, 60], &[22, 80])],
                local_ports: vec![3000, 8080, 9000],
            })
        );
    }

    #[test]
    fn text_that_is_not_an_address_or_a_port_names_where_it_is() {
        assert_eq!(
            parse("192.168.1.50\nprinter.local", ""),
            Err("Line 2: printer.local is not an IPv4 address".into())
        );
        assert_eq!(
            parse("192.168.1.50:", ""),
            Err("Line 1: no ports after the colon".into())
        );
        assert_eq!(
            parse("192.168.1.50:80,http", ""),
            Err("Line 1: http is not a port".into())
        );
        assert_eq!(parse("", "0"), Err("This computer: 0 is not a port".into()));
        assert_eq!(parse("", "70000"), Err("This computer: 70000 is not a port".into()));
    }

    #[test]
    fn the_header_says_what_the_bridge_reaches() {
        assert_eq!(summary(&Rules::default()), "Scope: the whole network");
        let narrowed = Rules {
            devices: vec![device([192, 168, 1, 50], &[554])],
            local_ports: vec![3000, 8080],
        };
        assert_eq!(summary(&narrowed), "Scope: 1 device · 2 ports on this computer");
    }

    #[test]
    fn refused_rules_leave_the_applied_ones_and_say_why() {
        let mut editor = Editor::default();
        let narrowed = Rules {
            devices: vec![device([192, 168, 1, 50], &[])],
            local_ports: Vec::new(),
        };
        editor.outcome(narrowed.clone(), Ok(()));
        assert_eq!(editor.applied, narrowed);
        editor.outcome(
            Rules::default(),
            Err("10.0.0.5 is not a device on the bridged network".into()),
        );
        assert_eq!(editor.applied, narrowed);
        assert_eq!(
            editor.error.as_deref(),
            Some("10.0.0.5 is not a device on the bridged network")
        );
        editor.reset();
        assert_eq!((editor.applied, editor.error), (Rules::default(), None));
    }
}
