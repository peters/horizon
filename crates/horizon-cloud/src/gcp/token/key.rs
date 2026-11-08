//! The RSA private key of a service-account document: the PKCS#8 PEM block that
//! `ring` signs with. The PEM and the DER decoded from it are wiped on drop, and
//! neither is ever printed.

use super::{Error, Result};
use base64::Engine as _;
use ring::{rand::SystemRandom, signature};
use zeroize::Zeroizing;

const BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const END: &str = "-----END PRIVATE KEY-----";

/// A service account's private key in its PKCS#8 PEM form.
pub(super) struct PrivateKey(Zeroizing<String>);

impl PrivateKey {
    /// Keeps `pem` once `ring` accepts it, so a broken key file fails where it is
    /// read instead of on the first request that needs a token.
    pub(super) fn new(pem: Zeroizing<String>) -> Result<Self> {
        let key = Self(pem);
        key.key_pair()?;
        Ok(key)
    }

    /// The signature over `message`, as base64url without padding.
    pub(super) fn sign(&self, message: &str) -> Result<String> {
        let key = self.key_pair()?;
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &signature::RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .map_err(|_| Error::PrivateKey)?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature))
    }

    fn key_pair(&self) -> Result<signature::RsaKeyPair> {
        signature::RsaKeyPair::from_pkcs8(&self.der()?).map_err(|_| Error::PrivateKey)
    }

    /// The DER that the PEM block carries. A block of any other kind is refused: it
    /// holds no PKCS#8 key that `ring` can read.
    fn der(&self) -> Result<Zeroizing<Vec<u8>>> {
        let body: Zeroizing<String> = Zeroizing::new(
            self.0
                .trim()
                .strip_prefix(BEGIN)
                .and_then(|rest| rest.strip_suffix(END))
                .ok_or(Error::PrivateKey)?
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .map(char::from)
                .collect(),
        );
        if body.is_empty() {
            return Err(Error::PrivateKey);
        }
        let mut der = Zeroizing::new(vec![0; body.len() / 4 * 3 + 3]);
        let written = base64::engine::general_purpose::STANDARD
            .decode_slice(body.as_bytes(), &mut der)
            .map_err(|_| Error::PrivateKey)?;
        der.truncate(written);
        Ok(der)
    }
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PrivateKey(<redacted>)")
    }
}
