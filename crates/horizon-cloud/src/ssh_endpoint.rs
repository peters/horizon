//! Validated SSH destinations. Resolution belongs to the bounded SSH transport.
use serde::{Deserialize, Serialize};
use std::{fmt, net::IpAddr, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct SshHost(String);

impl SshHost {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for SshHost {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.parse::<IpAddr>().is_ok() || valid_dns(&value) {
            Ok(Self(value))
        } else {
            Err("Invalid SSH hostname or IP address")
        }
    }
}
impl FromStr for SshHost {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}
impl From<SshHost> for String {
    fn from(host: SshHost) -> Self {
        host.0
    }
}
impl From<IpAddr> for SshHost {
    fn from(host: IpAddr) -> Self {
        Self(host.to_string())
    }
}
impl fmt::Display for SshHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
fn valid_dns(value: &str) -> bool {
    let name = value.strip_suffix('.').unwrap_or(value);
    !name.is_empty()
        && name.len() <= 253
        && name.bytes().any(|byte| byte.is_ascii_alphabetic())
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshEndpoint {
    host: SshHost,
    port: u16,
}
impl SshEndpoint {
    #[must_use]
    pub fn new(host: SshHost, port: u16) -> Option<Self> {
        (port != 0).then_some(Self { host, port })
    }
    #[must_use]
    pub fn host(&self) -> &SshHost {
        &self.host
    }
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_round_trip_without_resolving_and_reject_command_syntax() {
        for host in [
            "192.0.2.1",
            "2001:db8::1",
            "worker.example.invalid",
            "worker-1.example.invalid.",
            "localhost",
        ] {
            let parsed: SshHost = host.parse().unwrap();
            let encoded = serde_json::to_string(&parsed).unwrap();
            assert_eq!(serde_json::from_str::<SshHost>(&encoded).unwrap(), parsed);
            assert_eq!(parsed.as_str(), host);
        }
        for host in [
            "",
            "-oProxyCommand=x",
            "user@host",
            "host:22",
            "[::1]",
            "a..b",
            ".host",
            "host-",
            "a_b",
            "host\nProxyCommand x",
            "$(id)",
            "a/b",
            "999.999.999.999",
            "2001:bad::oops",
            "é.example",
        ] {
            assert!(host.parse::<SshHost>().is_err(), "{host:?}");
            assert!(serde_json::from_value::<SshHost>(serde_json::json!(host)).is_err());
        }
        assert!(format!("{}.example", "a".repeat(64)).parse::<SshHost>().is_err());
        assert!(SshEndpoint::new("localhost".parse().unwrap(), 0).is_none());
    }
}
