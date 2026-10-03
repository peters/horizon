use crate::{Error, Result, crypto};
use crypto_bigint::{
    Odd, U3072,
    modular::{FixedMontyForm, FixedMontyParams},
};
use zeroize::Zeroizing;

const MODULUS: Odd<U3072> = Odd::<U3072>::from_be_hex(include_str!("srp_group.txt"));
const PARAMS: FixedMontyParams<{ U3072::LIMBS }> = FixedMontyParams::new_vartime(MODULUS);

pub(crate) struct Exchange {
    pub(crate) public: Vec<u8>,
    pub(crate) proof: Vec<u8>,
    pub(crate) expected: Vec<u8>,
    pub(crate) secret: Zeroizing<Vec<u8>>,
}

fn integer(bytes: &[u8]) -> U3072 {
    let mut padded = Zeroizing::new([0; 384]);
    padded[384 - bytes.len()..].copy_from_slice(bytes);
    U3072::from_be_slice(padded.as_ref())
}
fn natural(bytes: &[u8]) -> &[u8] {
    &bytes[bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len())..]
}

pub(crate) fn exchange(pin: &str, salt: &[u8], server: &[u8]) -> Result<Exchange> {
    calculate(pin, salt, server, crypto::random::<32>()?.as_ref())
}

pub(crate) fn calculate(pin: &str, salt: &[u8], server: &[u8], private: &[u8]) -> Result<Exchange> {
    if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Protocol("PIN must contain four digits"));
    }
    if salt.len() != 16 || server.len() != 384 {
        return Err(Error::Protocol("invalid SRP parameters"));
    }
    if private.len() != 32 {
        return Err(Error::Protocol("invalid SRP private value"));
    }
    let modulus = *MODULUS.as_ref();
    let generator = U3072::from(5u8);
    let server_public = integer(server);
    if server_public == U3072::ZERO || server_public >= modulus {
        return Err(Error::Authentication);
    }
    let client_private = Zeroizing::new(integer(private));
    if *client_private == U3072::ZERO {
        return Err(Error::Authentication);
    }
    let montgomery = |value: &U3072| FixedMontyForm::new(value, &PARAMS);
    let public = montgomery(&generator).pow_bounded_exp(&*client_private, 256).retrieve();
    let identity = Zeroizing::new(crypto::hash(&[b"Pair-Setup:", pin.as_bytes()]));
    let password_hash = Zeroizing::new(crypto::hash(&[salt, &identity]));
    let password_exponent = Zeroizing::new(integer(&password_hash));
    let multiplier = integer(&crypto::hash(&[&modulus.to_be_bytes(), &generator.to_be_bytes()]));
    let scrambling = integer(&crypto::hash(&[&public.to_be_bytes(), &server_public.to_be_bytes()]));
    if scrambling == U3072::ZERO {
        return Err(Error::Authentication);
    }
    let verifier = Zeroizing::new(montgomery(&generator).pow_bounded_exp(&*password_exponent, 512));
    let base = Zeroizing::new(montgomery(&server_public).sub(&montgomery(&multiplier).mul(&verifier)));
    let exponent = Zeroizing::new(
        scrambling
            .wrapping_mul(&password_exponent)
            .wrapping_add(&client_private),
    );
    let premaster = Zeroizing::new(base.pow_bounded_exp(&*exponent, 1025).retrieve().to_be_bytes().to_vec());
    let secret = Zeroizing::new(crypto::hash(&[natural(premaster.as_ref())]));
    let n_hash = crypto::hash(&[&modulus.to_be_bytes()]);
    let g_hash = crypto::hash(&[&[5]]);
    let xor: Vec<_> = n_hash
        .iter()
        .zip(g_hash)
        .map(|(client_private, server_public)| client_private ^ server_public)
        .collect();
    let public_bytes = public.to_be_bytes();
    let raw_public = natural(&public_bytes);
    let proof = crypto::hash(&[
        &xor,
        &crypto::hash(&[b"Pair-Setup"]),
        salt,
        raw_public,
        natural(server),
        &secret,
    ]);
    let expected = crypto::hash(&[raw_public, &proof, &secret]);
    Ok(Exchange {
        public: public.to_be_bytes().to_vec(),
        proof,
        expected,
        secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bytes(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|part| u8::from_str_radix(std::str::from_utf8(part).expect("hex"), 16).expect("hex"))
            .collect()
    }
    #[test]
    fn independent_srp3072_sha512_vector() {
        let server = bytes(
            "5a4277047e3c3d8d1fb0dd06b512366d6509fdf0c4f7e3dfffbd1618822edfd6695917ee6ca08c595d11b8c38cf6df6a2f9b82c730e67550fb1c70301ae48010316e21155d12112a7b36a4599426e3f3385e44c34ae4fc6f42f09f1f868b34797997edffb9fb47972c617b4aa1066234f6788eff4d668c6335f6a0b4c74babf16ceddcc85f265ada66597a410ee754752aa9e53dd50c1cf426469bc5524cc3eb7ad94d873abebbc1e8b6d78bd1b15c84cd363274fd8ff614e93850aec57277ec9fe07b84071df8b152e6d6f2ee60b4f351ddb277a5b6ccbdf3c1be495039ff785093f64af2965626420e269f8d77722ce814cd6bf8cdd0b9950411bceb4599503b31d4e190c11bbb07bea4d9fe308e425d277abb34899b21ea89778d0f820f39d544ce099a636432fcf364931f55e9927e7d244c406ed9716f796be813f691b885099bcfdc369ba5cb9f8f07160d5a797a733f856ccb07f7c18f05f78856352cee41c38b09faac12ca3aa7639ffe20b27ef797ba2926cfd2754aebbf5976fe9a",
        );
        let proof = calculate(
            "0123",
            &(1..17).collect::<Vec<_>>(),
            &server,
            &(1..33).collect::<Vec<_>>(),
        )
        .expect("valid exchange");
        assert_eq!(
            proof.public.as_slice(),
            bytes(
                "bc0e7cf5dc3babf67dcedbb3b140aacc6cac43f4336b43bbd5de48d6ea7c8eda66924e354255225bccad9debe21182e6bb050f3ff3e6cfbb62c229379968c70ca436ad649a0b051373184215eef046f6f1f2256838f958581f6c7b2b85fa4afe326a0e8a951d4489305331aff88a136fd8d108bcc95fceb7e557c889c828bd23fb0702f053e1ca6470fb3c76bce4843fc005c7ea675740f8550212656cfc8919d9db805a434a68229e0d9dfe43fc16dc680a5ce74b77cf374353b05759bc1da3a9dabde30a4209381c87ca83d9483abdf66b86f9b1cbda9ad82c62712b87ce6fb7069b8fc8df344261821a06d0dc5106af76d4245f3f7737a94dbc484b415555dc401842d3011204553ba9f611b02bc38de26eba1a76bf8350205a62c436ba1c3c7c69d59318bd107fd1c1f5d846b3142e85a5d49e522655e020ed1bfe1e186cf923bf328f0b9b4c6a8aa3266ed9125bb98d63827110713be7803122ee4603c54ea31863ce4b10aff31f9073cf63b94733b4f066e72d4ec35687047d5d0db160"
            )
        );
        assert_eq!(
            proof.secret.as_slice(),
            bytes(
                "d770d29efd65f9bf55deef1ec12a7bf15e612ca98bc885360c0b68cb8fc4c677bfc75049d9d6283fd546f686f16f7d639eacffc4c3db6d249a10902f53aff5e3"
            )
        );
        assert_eq!(
            proof.proof.as_slice(),
            bytes(
                "605baac554707a2a8dbdaeccd26b83448575501f75fafa106afdb765c727e7e1939d2eeb9f6b50c75a440f85a183b2dc2c749aae581b1094698347cfa730f121"
            )
        );
        assert_eq!(
            proof.expected.as_slice(),
            bytes(
                "ff963105db618112bf83122a12c6625ab370b28c879f8646dca257245c08f4fb83e97afabc55b7d5a178e6b940ec790b19bb8ac3e876bbfb3dc0d48d45e02eec"
            )
        );
        assert!(calculate("0123", &[1; 16], &[0; 384], &[1; 32]).is_err());
        assert!(calculate("0123", &[1; 16], &[255; 384], &[1; 32]).is_err());
    }
}
