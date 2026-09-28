//! ACR grants may be empty even when authentication succeeds. Check the exact
//! repository's actions in the fresh HTTPS token response, never a saved JWT.
use super::{Auth, Material, Purpose};
use crate::cloud_runtime::{Cancellation, Error, Result};
use base64::{Engine as _, engine::general_purpose};
use serde::Deserialize;
use std::time::Duration;
use zeroize::Zeroizing;

const INVALID: &str = "ACR did not grant the required repository permissions";
const MAX_REPLY: u64 = 64 * 1024;

pub(super) fn verify(
    auth: &Auth,
    material: &Material,
    repository: &str,
    purpose: Purpose,
    cancel: &Cancellation,
) -> Result<()> {
    let (_, path) = repository.split_once('/').ok_or(Error::Invalid(INVALID))?;
    let host = super::super::credentials::registry_authority(repository);
    if !is_acr(&host) {
        return Ok(());
    }
    request(
        &format!("https://{host}/oauth2/token"),
        &host,
        path,
        auth,
        material,
        purpose,
        cancel,
    )
}

fn is_acr(host: &str) -> bool {
    [".azurecr.io", ".azurecr.cn", ".azurecr.us"]
        .iter()
        .any(|suffix| host.strip_suffix(suffix).is_some_and(|name| !name.is_empty()))
}

fn request(
    endpoint: &str,
    host: &str,
    repository: &str,
    auth: &Auth,
    material: &Material,
    purpose: Purpose,
    cancel: &Cancellation,
) -> Result<()> {
    cancel.check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let scope = format!("repository:{repository}:{}", purpose.actions().join(","));
    let pair = Zeroizing::new(format!("{}:{}", auth.username, material.secret.as_str()));
    let basic = Zeroizing::new(format!("Basic {}", general_purpose::STANDARD.encode(pair.as_bytes())));
    let answer = agent
        .get(endpoint)
        .query("service", host)
        .query("scope", &scope)
        .header("Authorization", basic.as_str())
        .call();
    cancel.check()?;
    let mut answer = answer.map_err(|_| Error::Invalid(INVALID))?;
    if answer.status().as_u16() != 200 {
        return Err(Error::Invalid(INVALID));
    }
    let bytes = Zeroizing::new(
        answer
            .body_mut()
            .with_config()
            .limit(MAX_REPLY)
            .read_to_vec()
            .map_err(|_| Error::Invalid(INVALID))?,
    );
    cancel.check()?;
    let grant: Grant<'_> = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid(INVALID))?;
    let token = grant.access_token.or(grant.token).ok_or(Error::Invalid(INVALID))?;
    check_grant(token, host, repository, purpose)
}

#[derive(Deserialize)]
struct Grant<'a> {
    #[serde(borrow)]
    token: Option<&'a str>,
    #[serde(borrow)]
    access_token: Option<&'a str>,
}

#[derive(Deserialize)]
struct Claims<'a> {
    aud: &'a str,
    exp: i64,
    nbf: Option<i64>,
    #[serde(borrow)]
    access: Vec<Access<'a>>,
}

#[derive(Deserialize)]
struct Access<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    name: &'a str,
    actions: Vec<&'a str>,
}

fn check_grant(token: &str, host: &str, repository: &str, purpose: Purpose) -> Result<()> {
    let mut parts = token.split('.');
    let _header = parts.next().ok_or(Error::Invalid(INVALID))?;
    let payload = parts.next().ok_or(Error::Invalid(INVALID))?;
    if parts.next().is_none_or(str::is_empty) || parts.next().is_some() {
        return Err(Error::Invalid(INVALID));
    }
    let bytes = Zeroizing::new(
        general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| Error::Invalid(INVALID))?,
    );
    let claims: Claims<'_> = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid(INVALID))?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if claims.aud != host || claims.exp <= now || claims.nbf.is_some_and(|start| start > now) {
        return Err(Error::Invalid(INVALID));
    }
    // The HTTPS response is issuer evidence, not independently verified JWT
    // authentication. The registry still validates the token on push/pull, and
    // the immutable image must still pass the separate worker pull check.
    if purpose.actions().iter().all(|required| {
        claims
            .access
            .iter()
            .any(|grant| grant.kind == "repository" && grant.name == repository && grant.actions.contains(required))
    }) {
        Ok(())
    } else {
        Err(Error::Invalid(INVALID))
    }
}

#[cfg(test)]
mod tests;
