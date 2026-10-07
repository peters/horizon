//! Cast Streaming frame encryption: AES-128 in counter mode with a per-session
//! key. Each frame starts its counter at the session's IV mask with the frame
//! ID, big-endian, mixed into bytes 8..12.
use aes::{
    Aes128,
    cipher::{BlockCipherEncrypt, KeyInit},
};

pub(crate) struct FrameCipher {
    cipher: Aes128,
    iv_mask: [u8; 16],
}

impl FrameCipher {
    pub(crate) fn new(key: [u8; 16], iv_mask: [u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(&key.into()),
            iv_mask,
        }
    }

    /// Encrypts `data` in place as frame `frame_id`. Counter mode is its own
    /// inverse, so the same call decrypts.
    pub(crate) fn apply(&self, frame_id: u32, data: &mut [u8]) {
        let mut nonce = self.iv_mask;
        for (byte, id) in nonce[8..12].iter_mut().zip(frame_id.to_be_bytes()) {
            *byte ^= id;
        }
        let mut counter = u128::from_be_bytes(nonce);
        for chunk in data.chunks_mut(16) {
            let mut block = counter.to_be_bytes().into();
            self.cipher.encrypt_block(&mut block);
            for (byte, key) in chunk.iter_mut().zip(block.iter()) {
                *byte ^= key;
            }
            counter = counter.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0; N];
        for (at, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn frame_zero_matches_the_nist_counter_mode_vectors() {
        // NIST SP 800-38A, F.5.1 CTR-AES128.Encrypt: frame 0 leaves the mask as the counter.
        let cipher = FrameCipher::new(
            hex("2b7e151628aed2a6abf7158809cf4f3c"),
            hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"),
        );
        let mut data: [u8; 32] = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        cipher.apply(0, &mut data);
        assert_eq!(
            data,
            hex::<32>("874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff")
        );
        cipher.apply(0, &mut data);
        assert_eq!(
            data,
            hex::<32>("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51")
        );
    }

    #[test]
    fn the_frame_id_moves_the_counter_start() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let mask = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut shifted = mask;
        for (byte, id) in shifted[8..12].iter_mut().zip(0x0102_0304u32.to_be_bytes()) {
            *byte ^= id;
        }
        let mut by_id = [0u8; 20];
        FrameCipher::new(key, mask).apply(0x0102_0304, &mut by_id);
        let mut by_mask = [0u8; 20];
        FrameCipher::new(key, shifted).apply(0, &mut by_mask);
        assert_eq!(by_id, by_mask);
        assert_ne!(by_id, [0; 20]);
    }
}
