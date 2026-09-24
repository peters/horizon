use super::{Auth, Cancellation, Error, Result, parse_expiry};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path, time::Duration};
use zeroize::Zeroizing;

pub(super) struct Material {
    pub secret: Zeroizing<String>,
    encoded: Zeroizing<String>,
    pub fingerprint: String,
    pub config: tempfile::TempDir,
}

impl Material {
    pub fn load(auth: &Auth, repository: &str, source: Option<&Path>) -> Result<Self> {
        auth.validate()?;
        auth.check_expiry()?;
        let path = auth.secret_file.canonicalize()?;
        if source.is_some_and(|source| path.starts_with(source)) {
            return Err(Error::Invalid(
                "Registry secrets must be outside the source and build context",
            ));
        }
        super::super::settings::validate_private_key_file(&path)?;
        let secret = Zeroizing::new(std::fs::read_to_string(&path)?.trim().to_owned());
        if secret.bytes().any(|byte| byte <= 32 || byte >= 127) {
            return Err(Error::Invalid("Invalid registry credential"));
        }
        let fingerprint = fingerprint(secret.as_bytes());
        let encoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .encode(Zeroizing::new(format!("{}:{}", auth.username, secret.as_str())).as_bytes()),
        );
        let config = tempfile::Builder::new().prefix("horizon-registry-").tempdir()?;
        let mut file = tempfile::NamedTempFile::new_in(config.path())?;
        let host = docker_auth_key(repository);
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({"auths": {host: {"auth": encoded.as_str()}}}),
        )
        .map_err(|_| Error::Json)?;
        file.flush()?;
        file.persist(config.path().join("config.json"))
            .map_err(|error| error.error)?;
        Ok(Self {
            secret,
            encoded,
            fingerprint,
            config,
        })
    }

    pub fn redactions(&self) -> Vec<String> {
        vec![self.secret.to_string(), self.encoded.to_string()]
    }

    pub fn verify_scope(&self, repository: &str, cancel: &Cancellation) -> Result<Option<String>> {
        cancel.check()?;
        let mut observed_expiry = None;
        if is_github_registry(repository) {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .max_redirects(0)
                .http_status_as_error(false)
                .build();
            let auth = Zeroizing::new(format!("Bearer {}", self.secret.as_str()));
            let response = ureq::Agent::new_with_config(config)
                .get("https://api.github.com/user")
                .header("User-Agent", "Horizon-registry-validation")
                .header("Authorization", auth.as_str())
                .call()
                .map_err(|_| Error::Invalid("Registry token scope verification failed"))?;
            if response.status().as_u16() != 200 {
                return Err(Error::Invalid("Registry token is expired, revoked or unauthorized"));
            }
            let scopes = response
                .headers()
                .get("x-oauth-scopes")
                .and_then(|value| value.to_str().ok());
            verify_github_scopes(scopes)?;
            if let Some(expiry) = response
                .headers()
                .get("github-authentication-token-expiration")
                .and_then(|value| value.to_str().ok())
            {
                // GitHub uses a space before the UTC time in this response header.
                let expiry = expiry.trim_end_matches(" UTC").replace(' ', "T");
                let expiry = if expiry.ends_with('Z') || expiry.contains('+') {
                    expiry
                } else {
                    format!("{expiry}Z")
                };
                observed_expiry = Some(expiry.clone());
                if parse_expiry(&expiry)? <= time::OffsetDateTime::now_utc().unix_timestamp() {
                    return Err(Error::Invalid("Registry token has expired"));
                }
            }
        }
        cancel.check()?;
        Ok(observed_expiry)
    }
}

pub(super) fn fingerprint(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}

pub(super) fn ensure_separate(
    pull: &Auth,
    material: &Material,
    publish: Option<&Auth>,
    source: Option<&Path>,
) -> Result<()> {
    let Some(publish) = publish else {
        return Ok(());
    };
    if pull.secret_file == publish.secret_file {
        return Err(Error::Invalid("Use different push and pull credential files"));
    }
    match std::fs::metadata(&publish.secret_file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    if source.is_some_and(|source| {
        publish
            .secret_file
            .canonicalize()
            .is_ok_and(|path| path.starts_with(source))
    }) {
        return Err(Error::Invalid(
            "Registry secrets must be outside the source and build context",
        ));
    }
    super::super::settings::validate_private_key_file(&publish.secret_file)?;
    let push_secret = Zeroizing::new(std::fs::read_to_string(&publish.secret_file)?);
    if fingerprint(push_secret.trim().as_bytes()) == material.fingerprint {
        return Err(Error::Invalid("Use different push and pull credentials"));
    }
    Ok(())
}

pub(super) fn verify_github_scopes(scopes: Option<&str>) -> Result<()> {
    let scopes: Vec<_> = scopes
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .collect();
    if scopes != ["read:packages"] {
        return Err(Error::Invalid(
            "Worker pulls require a dedicated classic token with only read:packages; broader or unknown grants are refused",
        ));
    }
    Ok(())
}

// Issuer policy follows the DNS hostname, including equivalent case, port and FQDN spellings.
pub(super) fn registry_host(repository: &str) -> String {
    let authority = repository.split('/').next().unwrap_or_default();
    let host = authority
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "index.docker.io" {
        "docker.io".into()
    } else {
        host
    }
}

pub(super) fn is_github_registry(repository: &str) -> bool {
    registry_host(repository) == "ghcr.io"
}

pub(super) fn registry_authority(repository: &str) -> String {
    let authority = repository.split('/').next().unwrap_or_default();
    let host = registry_host(repository);
    match authority.split_once(':') {
        Some((_, port)) => match port.parse::<u16>() {
            Ok(443) => host,
            Ok(port) => format!("{host}:{port}"),
            Err(_) => format!("{host}:{port}"),
        },
        None => host,
    }
}

pub(super) fn docker_auth_key(repository: &str) -> &str {
    match repository.split('/').next().unwrap_or_default() {
        "docker.io" | "index.docker.io" => "https://index.docker.io/v1/",
        _ => repository.split('/').next().unwrap_or_default(),
    }
}
