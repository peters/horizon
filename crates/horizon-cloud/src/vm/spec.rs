//! The spec helpers that any virtual-machine provider's host plan needs: the Chromium
//! shared memory, the registry an image pulls from and a throwaway SSH key. No provider
//! owns these, so none reaches into another for them.
use crate::CloudError;

/// A quarter of the worker's memory for Chromium's shared memory, within the host's limits.
#[must_use]
pub(crate) fn shared_memory_gb(memory_gb: u16) -> u8 {
    u8::try_from((memory_gb / 4).clamp(1, 16)).unwrap_or(1)
}

/// The registry an image reference pulls from, as Docker resolves it: the first
/// path component when it names a host, otherwise Docker Hub.
#[must_use]
pub(crate) fn image_registry(image: &str) -> String {
    match image.split_once('/') {
        Some((first, _)) if first.contains(['.', ':']) || first == "localhost" => registry_host(first),
        _ => registry_host("docker.io"),
    }
}

/// A registry host in one spelling, folding together the Docker Hub aliases the
/// host configuration stores under Docker Hub's credential key.
#[must_use]
pub(crate) fn registry_host(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "index.docker.io" | "registry-1.docker.io" => "docker.io".into(),
        _ => host,
    }
}

/// An Ed25519 public key in OpenSSH form whose private half is discarded at once.
/// # Errors
/// Fails without system randomness.
pub fn throwaway_public_key() -> Result<String, CloudError> {
    use base64::Engine as _;
    use ring::rand::SecureRandom as _;
    let mut seed = zeroize::Zeroizing::new([0_u8; 32]);
    // The message names Hetzner because this helper began there. It stays byte-identical
    // for the callers that already show it.
    ring::rand::SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| CloudError::Invalid("No randomness for a Hetzner SSH key"))?;
    let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let mut wire = Vec::with_capacity(51);
    for field in [b"ssh-ed25519".as_slice(), public.as_slice()] {
        wire.extend_from_slice(&u32::try_from(field.len()).unwrap_or(0).to_be_bytes());
        wire.extend_from_slice(field);
    }
    Ok(format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(wire)
    ))
}
