use super::ArtifactDigest;
use sha2::{Digest, Sha256};

impl ArtifactDigest {
    /// Hash already-owned bytes without I/O. This does not authenticate or authorize their source.
    #[must_use]
    pub fn sha256(bytes: &[u8]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_byte_vectors_with_canonical_lowercase_hex() {
        for (bytes, expected) in [
            (
                b"".as_slice(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc".as_slice(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                [0, 255, 128, 13, 10].as_slice(),
                "6171db06a1c89b1ff8ab77e479d5df976ccd88a7b63567b4b18f413649e12ff3",
            ),
        ] {
            let digest = ArtifactDigest::sha256(bytes);
            assert_eq!(digest.as_str(), expected);
            assert_eq!(ArtifactDigest::parse_sha256(expected), Ok(digest));
        }
        assert_ne!(ArtifactDigest::sha256(b"a\0b"), ArtifactDigest::sha256(b"ab"));
    }
}
