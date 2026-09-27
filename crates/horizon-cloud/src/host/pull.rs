//! Checks that a registry login can read an image before a server is paid for.
//! A host that cannot pull its image never becomes ready, so an expired or revoked
//! pull token would otherwise surface only as a readiness timeout. The check asks
//! the registry the way `docker pull` does, through the OCI distribution API: the
//! manifest request is challenged, a token is requested with the login, and the
//! manifest is requested again with it. Only a definite answer refuses: the login
//! rejected, or the image missing. A registry that cannot be reached or answers
//! otherwise leaves the decision to the host's own pull.
use super::RegistryLogin;
use crate::{Cancellation, CloudError};
use base64::Engine as _;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
/// Manifest kinds a Docker host accepts; a registry answers for the image as it stores it.
const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json";
const REFUSED: &str = "The registry refused the pull credential for this image; it may have expired or been revoked. Renew it before deploying";
const MISSING: &str = "The registry has no such image; check the image reference before deploying";
const OTHER_REGISTRY: &str = "The pull login names a different registry than the image";

/// Checks that `login` can read `image`, an image reference such as
/// `example.azurecr.io/team/worker@sha256:...`.
/// The login is sent only to the image's own registry and its token service.
/// # Errors
/// Refuses a login for another registry than the image's before any request, a
/// login the registry rejects and an image it does not have, and reports
/// `CloudError::Cancelled` once `cancel` is cancelled.
pub fn verify_pull(login: &RegistryLogin, image: &str, cancel: &Cancellation) -> Result<(), CloudError> {
    verify_pull_at(login, image, cancel, "https")
}

fn verify_pull_at(login: &RegistryLogin, image: &str, cancel: &Cancellation, scheme: &str) -> Result<(), CloudError> {
    let Some(reference) = Reference::parse(image) else {
        return Ok(());
    };
    if api_host(&login.server) != reference.host {
        return Err(CloudError::Invalid(OTHER_REGISTRY));
    }
    cancel.check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let manifest = format!(
        "{scheme}://{}/v2/{}/manifests/{}",
        reference.host, reference.repository, reference.target
    );
    let pair = zeroize::Zeroizing::new(format!("{}:{}", login.username, login.password.value()));
    let encoded = zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(pair.as_bytes()));
    let basic = zeroize::Zeroizing::new(format!("Basic {}", encoded.as_str()));
    let Ok(challenged) = agent.head(&manifest).header("Accept", ACCEPT).call() else {
        return Ok(());
    };
    let authorization = match challenged.status().as_u16() {
        401 => {
            let challenge = challenged
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            match Challenge::parse(challenge) {
                Some(Challenge::Basic) => basic.clone(),
                Some(Challenge::Bearer { realm, service }) => {
                    // Credentials go only to a token service reached over the same scheme.
                    if !realm.starts_with(&format!("{scheme}://")) {
                        return Ok(());
                    }
                    cancel.check()?;
                    let scope = format!("repository:{}:pull", reference.repository);
                    let mut request = agent.get(&realm).query("scope", &scope);
                    if let Some(service) = &service {
                        request = request.query("service", service);
                    }
                    let Ok(mut answer) = request.header("Authorization", basic.as_str()).call() else {
                        return Ok(());
                    };
                    match answer.status().as_u16() {
                        200 => {}
                        401 | 403 => return Err(CloudError::Invalid(REFUSED)),
                        _ => return Ok(()),
                    }
                    let Some(token) = answer
                        .body_mut()
                        .read_json::<serde_json::Value>()
                        .ok()
                        .and_then(|body| {
                            body.get("token")
                                .or_else(|| body.get("access_token"))
                                .and_then(serde_json::Value::as_str)
                                .map(|token| zeroize::Zeroizing::new(token.to_owned()))
                        })
                    else {
                        return Ok(());
                    };
                    zeroize::Zeroizing::new(format!("Bearer {}", token.as_str()))
                }
                None => return Ok(()),
            }
        }
        // Public (200), so the login is not needed to read it, or indefinite: a
        // registry may answer 404 to an anonymous request to hide a private image.
        _ => return Ok(()),
    };
    cancel.check()?;
    let Ok(read) = agent
        .head(&manifest)
        .header("Accept", ACCEPT)
        .header("Authorization", authorization.as_str())
        .call()
    else {
        return Ok(());
    };
    match read.status().as_u16() {
        401 | 403 => Err(CloudError::Invalid(REFUSED)),
        404 => Err(CloudError::Invalid(MISSING)),
        _ => Ok(()),
    }
}

/// The registry API host a login or image names, lowercase, with Docker Hub's
/// names folded into its API host.
fn api_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    if matches!(host.as_str(), "docker.io" | "index.docker.io" | "registry-1.docker.io") {
        "registry-1.docker.io".into()
    } else {
        host
    }
}

/// Where an image's manifest is served, as Docker resolves the reference.
#[derive(Debug, PartialEq, Eq)]
struct Reference {
    /// The registry API host, with Docker Hub's names folded into its API host.
    host: String,
    repository: String,
    /// A digest, or a tag, `latest` when none is named.
    target: String,
}

impl Reference {
    fn parse(image: &str) -> Option<Self> {
        let (name, target) = match image.split_once('@') {
            Some((name, digest)) => (name, digest.to_owned()),
            None => match image.rsplit_once(':') {
                Some((name, tag)) if !tag.contains('/') => (name, tag.to_owned()),
                _ => (image, "latest".to_owned()),
            },
        };
        let (host, repository) = match name.split_once('/') {
            Some((first, rest)) if first.contains(['.', ':']) || first == "localhost" => {
                (first.to_ascii_lowercase(), rest.to_owned())
            }
            _ => ("docker.io".to_owned(), name.to_owned()),
        };
        let docker_hub = matches!(host.as_str(), "docker.io" | "index.docker.io" | "registry-1.docker.io");
        let repository = if docker_hub && !repository.contains('/') {
            format!("library/{repository}")
        } else {
            repository
        };
        let safe = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':' | b'+'))
        };
        (safe(&host) && safe(&repository) && safe(&target)).then(|| Self {
            host: api_host(&host),
            repository,
            target,
        })
    }
}

/// How a registry asks to be authenticated, from its `WWW-Authenticate` header.
#[derive(Debug, PartialEq, Eq)]
enum Challenge {
    Basic,
    Bearer { realm: String, service: Option<String> },
}

impl Challenge {
    fn parse(header: &str) -> Option<Self> {
        let (scheme, parameters) = header.trim().split_once(' ').unwrap_or((header.trim(), ""));
        if scheme.eq_ignore_ascii_case("basic") {
            return Some(Self::Basic);
        }
        if !scheme.eq_ignore_ascii_case("bearer") {
            return None;
        }
        let mut realm = None;
        let mut service = None;
        let mut rest = parameters.trim();
        while !rest.is_empty() {
            let (key, after) = rest.split_once('=')?;
            let after = after.trim_start();
            let (value, remaining) = if let Some(quoted) = after.strip_prefix('"') {
                let end = quoted.find('"')?;
                (&quoted[..end], &quoted[end + 1..])
            } else {
                after
                    .split_once(',')
                    .map_or((after, ""), |(value, remaining)| (value, remaining))
            };
            match key.trim().to_ascii_lowercase().as_str() {
                "realm" => realm = Some(value.to_owned()),
                "service" => service = Some(value.to_owned()),
                _ => {}
            }
            rest = remaining.trim_start().trim_start_matches(',').trim_start();
        }
        Some(Self::Bearer { realm: realm?, service })
    }
}

#[cfg(test)]
mod tests;
