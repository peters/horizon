use serde::{Deserialize, Serialize};

use crate::{Error, Result, model::parse_key};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfiguration {
    pub hostname: String,
    pub contact: String,
    pub certificate_directory: String,
    pub allowed_keys: Vec<String>,
}

impl RelayConfiguration {
    /// Generate the upstream iroh-relay 1.3 TOML. The relay carries ciphertext;
    /// application grants remain enforced by endpoints.
    ///
    /// # Errors
    /// Returns an error for missing TLS settings or an invalid endpoint allowlist.
    pub fn to_toml(&self) -> Result<String> {
        if self.hostname.is_empty()
            || self.contact.is_empty()
            || self.certificate_directory.is_empty()
            || self.allowed_keys.is_empty()
            || !self
                .hostname
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        {
            return Err(Error::InvalidConfiguration(
                "relay hostname, contact, certificate directory and allowlist are required".into(),
            ));
        }
        let keys = self
            .allowed_keys
            .iter()
            .map(|key| parse_key(key).and_then(|key| serde_json::to_string(&key.to_string()).map_err(Error::from)))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "enable_relay = true\nenable_quic_addr_discovery = false\nenable_metrics = false\nhttp_bind_addr = \"127.0.0.1:8080\"\naccess.allowlist = [{keys}]\n\n[tls]\nhttps_bind_addr = \"0.0.0.0:443\"\ncert_mode = \"LetsEncrypt\"\nhostname = {}\ncontact = {}\ncert_dir = {}\nprod_tls = true\n",
            serde_json::to_string(&self.hostname)?,
            serde_json::to_string(&self.contact)?,
            serde_json::to_string(&self.certificate_directory)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn configuration() -> RelayConfiguration {
        RelayConfiguration {
            hostname: "relay-1.example.com".into(),
            contact: "mailto:operator@example.com\n[access]\nallowlist = []".into(),
            certificate_directory: "C:\\relay\\\"certificates\"\ncache".into(),
            allowed_keys: [1, 2]
                .into_iter()
                .map(|byte| iroh::SecretKey::from_bytes(&[byte; 32]).public().to_string())
                .collect(),
        }
    }

    #[test]
    fn generated_toml_keeps_explicit_allowlist_tls_and_escaped_values() -> Result<()> {
        let configuration = configuration();
        let document = configuration
            .to_toml()?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| Error::InvalidConfiguration(error.to_string()))?;
        assert_eq!(
            document.iter().map(|(key, _)| key).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "access",
                "enable_metrics",
                "enable_quic_addr_discovery",
                "enable_relay",
                "http_bind_addr",
                "tls",
            ])
        );
        assert_eq!(document["enable_relay"].as_bool(), Some(true));
        assert_eq!(document["enable_quic_addr_discovery"].as_bool(), Some(false));
        assert_eq!(document["enable_metrics"].as_bool(), Some(false));
        assert_eq!(document["http_bind_addr"].as_str(), Some("127.0.0.1:8080"));
        let access = document["access"]
            .as_table()
            .ok_or_else(|| Error::InvalidConfiguration("access table missing".into()))?;
        assert_eq!(access.iter().map(|(key, _)| key).collect::<Vec<_>>(), ["allowlist"]);
        let allowlist = document["access"]["allowlist"]
            .as_array()
            .ok_or_else(|| Error::InvalidConfiguration("allowlist array missing".into()))?;
        assert_eq!(
            allowlist
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .collect::<Vec<_>>(),
            configuration
                .allowed_keys
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(allowlist.len(), configuration.allowed_keys.len());
        let tls = document["tls"]
            .as_table()
            .ok_or_else(|| Error::InvalidConfiguration("TLS table missing".into()))?;
        assert_eq!(
            tls.iter().map(|(key, _)| key).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "cert_dir",
                "cert_mode",
                "contact",
                "hostname",
                "https_bind_addr",
                "prod_tls",
            ])
        );
        assert_eq!(tls["https_bind_addr"].as_str(), Some("0.0.0.0:443"));
        assert_eq!(tls["cert_mode"].as_str(), Some("LetsEncrypt"));
        assert_eq!(tls["prod_tls"].as_bool(), Some(true));
        assert_eq!(tls["hostname"].as_str(), Some(configuration.hostname.as_str()));
        assert_eq!(tls["contact"].as_str(), Some(configuration.contact.as_str()));
        assert_eq!(
            tls["cert_dir"].as_str(),
            Some(configuration.certificate_directory.as_str())
        );
        Ok(())
    }

    #[test]
    fn relay_configuration_rejects_missing_allowlist_and_hostname_injection() {
        let mut configuration = configuration();
        configuration.allowed_keys.clear();
        assert!(configuration.to_toml().is_err());
        configuration.allowed_keys.push("invalid-key".into());
        assert!(configuration.to_toml().is_err());
        configuration = self::configuration();
        configuration.hostname.push_str("\"\naccess.allowlist = []");
        assert!(configuration.to_toml().is_err());
        configuration = self::configuration();
        configuration.contact.clear();
        assert!(configuration.to_toml().is_err());
        configuration = self::configuration();
        configuration.certificate_directory.clear();
        assert!(configuration.to_toml().is_err());
    }
}
