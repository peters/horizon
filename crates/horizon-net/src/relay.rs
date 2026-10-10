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
