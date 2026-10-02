use crate::{Error, Result};
use ring::{
    aead, digest, hkdf,
    rand::{SecureRandom, SystemRandom},
};
use zeroize::Zeroizing;

pub(crate) fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0; N]);
    SystemRandom::new()
        .fill(bytes.as_mut())
        .map_err(|_| Error::Authentication)?;
    Ok(bytes)
}

pub(crate) fn hash(parts: &[&[u8]]) -> Vec<u8> {
    let mut ctx = digest::Context::new(&digest::SHA512);
    for part in parts {
        ctx.update(part);
    }
    ctx.finish().as_ref().to_vec()
}

struct KeyLength;
impl hkdf::KeyType for KeyLength {
    fn len(&self) -> usize {
        32
    }
}

pub(crate) fn derive(secret: &[u8], salt: &str, label: &str) -> Result<Zeroizing<[u8; 32]>> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA512, salt.as_bytes()).extract(secret);
    let labels = [label.as_bytes()];
    let okm = prk.expand(&labels, KeyLength).map_err(|_| Error::Authentication)?;
    let mut key = Zeroizing::new([0; 32]);
    okm.fill(key.as_mut()).map_err(|_| Error::Authentication)?;
    Ok(key)
}

pub(crate) fn key(bytes: &[u8]) -> Result<aead::LessSafeKey> {
    aead::UnboundKey::new(&aead::CHACHA20_POLY1305, bytes)
        .map(aead::LessSafeKey::new)
        .map_err(|_| Error::Authentication)
}

pub(crate) fn nonce(tail: [u8; 8]) -> aead::Nonce {
    let mut bytes = [0; 12];
    bytes[4..].copy_from_slice(&tail);
    aead::Nonce::assume_unique_for_key(bytes)
}

pub(crate) fn seal(key: &aead::LessSafeKey, tail: [u8; 8], aad: &[u8], mut data: Vec<u8>) -> Result<Vec<u8>> {
    key.seal_in_place_append_tag(nonce(tail), aead::Aad::from(aad), &mut data)
        .map_err(|_| Error::Authentication)?;
    Ok(data)
}

pub(crate) fn open(key: &aead::LessSafeKey, tail: [u8; 8], aad: &[u8], mut data: Vec<u8>) -> Result<Vec<u8>> {
    let len = key
        .open_in_place(nonce(tail), aead::Aad::from(aad), &mut data)
        .map_err(|_| Error::Authentication)?
        .len();
    data.truncate(len);
    Ok(data)
}
