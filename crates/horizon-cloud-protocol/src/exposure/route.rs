//! Shared route facts. Neither syntax nor a retained URL publishes a service.
use super::{Error, ExposureMode, ExposureRequest, RESERVED_INGRESS_END, RESERVED_INGRESS_START, dns_name};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Route {
    Gate {},
    FunnelHttps { https: u16 },
}
impl Default for Route {
    fn default() -> Self {
        Self::Gate {}
    }
}
impl Route {
    /// # Errors
    /// Funnel is explicit Public HTTPS only; refusal never selects a raw fallback.
    pub fn validate_for(self, policy: &ExposureRequest) -> Result<(), Error> {
        policy.validate()?;
        match self {
            Self::Gate {} => Ok(()),
            Self::FunnelHttps { https }
                if policy.mode == ExposureMode::Public && matches!(https, 443 | 8443 | 10000) =>
            {
                Ok(())
            }
            Self::FunnelHttps { .. } => Err(Error::Route),
        }
    }
    #[must_use]
    pub const fn accepts_ingress(self, port: u16) -> bool {
        match self {
            Self::Gate {} => port >= RESERVED_INGRESS_START && port <= RESERVED_INGRESS_END,
            Self::FunnelHttps { https } => matches!(https, 443 | 8443 | 10000) && port == https,
        }
    }
    /// # Errors
    /// Tailnet TCP endpoints require an actual reserved virtual mapping and the
    /// original daemon's canonical name. These facts alone do not prove readiness.
    pub fn observed_url(self, original_dns: &str, tailnet_ingress: Option<u16>) -> Result<Option<String>, Error> {
        match (self, tailnet_ingress) {
            (Self::Gate {}, Some(port))
                if (RESERVED_INGRESS_START..=RESERVED_INGRESS_END).contains(&port)
                    && dns_name(original_dns).is_ok_and(|name| name == original_dns) =>
            {
                Ok(Some(format!("tcp://{original_dns}:{port}")))
            }
            (Self::Gate {}, None) => Ok(None),
            (Self::FunnelHttps { .. }, None) => self.url(original_dns),
            _ => Err(Error::Route),
        }
    }
    /// # Errors
    /// A URL is comparison facts from the original canonical daemon name, not readiness.
    pub fn url(self, original_dns: &str) -> Result<Option<String>, Error> {
        match self {
            Self::Gate {} => Ok(None),
            Self::FunnelHttps { https } => {
                if !matches!(https, 443 | 8443 | 10000)
                    || !dns_name(original_dns).is_ok_and(|name| name == original_dns)
                {
                    return Err(Error::Route);
                }
                Ok(Some(if https == 443 {
                    format!("https://{original_dns}/")
                } else {
                    format!("https://{original_dns}:{https}/")
                }))
            }
        }
    }
}
