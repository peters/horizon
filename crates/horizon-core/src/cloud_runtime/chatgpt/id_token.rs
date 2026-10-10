//! ID-token validation: signature against `OpenAI`'s published JWKS, plus issuer,
//! audience, validity window and nonce checks. A token that does not validate is an error,
//! never a warning.
use super::{CONFIG_URL, Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RSA_PSS_2048_8192_SHA256, RsaPublicKeyComponents,
    UnparsedPublicKey,
};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const ISSUER: &str = "https://auth.openai.com";
const ISSUED_AT_LEEWAY_SECONDS: i64 = 60;

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Key>,
}

#[derive(Deserialize)]
pub(super) struct Key {
    #[serde(rename = "kty")]
    typ: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
    #[serde(default)]
    crv: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    #[serde(default)]
    azp: Option<String>,
    exp: i64,
    iat: i64,
    #[serde(default)]
    nbf: Option<i64>,
    sub: String,
    nonce: String,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn matches(&self, client_id: &str) -> bool {
        match self {
            Self::One(audience) => audience == client_id,
            // No additional audiences are configured as trusted for this client.
            Self::Many(audiences) => audiences.len() == 1 && audiences[0] == client_id,
        }
    }
}

/// Validates `token` against the published JWKS and returns its verified identity.
/// # Errors
/// The token, its signature, its issuer, audience, expiry or nonce did not validate.
pub(super) fn validate(token: &str, client_id: &str, nonce: &str) -> Result<(String, Option<String>)> {
    let discovery: Discovery = fetch_json(CONFIG_URL)?;
    let jwks: Jwks = fetch_json(&discovery.jwks_uri)?;
    validate_with_jwks(token, client_id, nonce, &jwks.keys)
}

/// The validation a test or a cached JWKS can run without a network round trip.
/// # Errors
/// The token, its signature, its issuer, audience, expiry or nonce did not validate.
pub(super) fn validate_with_jwks(
    token: &str,
    client_id: &str,
    nonce: &str,
    keys: &[Key],
) -> Result<(String, Option<String>)> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| i64::try_from(age.as_secs()).unwrap_or(0));
    validate_with_jwks_at(token, client_id, nonce, keys, now)
}

fn validate_with_jwks_at(
    token: &str,
    client_id: &str,
    nonce: &str,
    keys: &[Key],
    now: i64,
) -> Result<(String, Option<String>)> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Malformed);
    };
    let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| Error::Malformed)?)
        .map_err(|_| Error::Malformed)?;
    let payload = zeroize::Zeroizing::new(URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| Error::Malformed)?);
    let claims: Claims = super::response::parse(&payload, "identity claims")?;
    let signature = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|_| Error::Malformed)?;

    if claims.iss != ISSUER {
        return Err(Error::IdToken);
    }
    if !claims.aud.matches(client_id) || claims.azp.as_deref().is_some_and(|party| party != client_id) {
        return Err(Error::IdToken);
    }
    if claims.nonce != nonce {
        return Err(Error::IdToken);
    }
    // Expiry and not-before remain strict; issuance allows at most one minute of clock skew.
    if claims.exp <= now
        || claims.nbf.is_some_and(|not_before| now < not_before)
        || claims.iat > now.saturating_add(ISSUED_AT_LEEWAY_SECONDS)
    {
        return Err(Error::IdToken);
    }

    let kid = header
        .kid
        .as_deref()
        .filter(|kid| !kid.is_empty())
        .ok_or(Error::IdToken)?;
    let key = keys
        .iter()
        .find(|key| key.kid.as_deref() == Some(kid))
        .ok_or(Error::IdToken)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    verify(&header.alg, key, signing_input.as_bytes(), &signature)?;
    Ok((claims.sub, claims.email))
}

/// GETs a JSON document, refusing non-2xx answers.
fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let endpoint = super::provider_endpoint(url)?;
    let response = ureq::get(endpoint.as_str())
        .config()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| Error::Provider(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status.as_u16()) {
        return Err(Error::Provider(format!("the sign-in service answered {status}")));
    }
    super::response::read(response.into_body(), "identity discovery or signing keys")
}

fn verify(alg: &str, key: &Key, signing_input: &[u8], signature: &[u8]) -> Result<()> {
    match (alg, key.typ.as_str()) {
        ("RS256" | "PS256", "RSA") => {
            let (Some(n), Some(e)) = (key.n.as_ref(), key.e.as_ref()) else {
                return Err(Error::IdToken);
            };
            let n = URL_SAFE_NO_PAD.decode(n).map_err(|_| Error::IdToken)?;
            let e = URL_SAFE_NO_PAD.decode(e).map_err(|_| Error::IdToken)?;
            // ring exposes a 2048-bit primitive verifier; OpenAI's JWKS publishes 2048-bit RSA.
            let alg = if alg == "RS256" {
                &RSA_PKCS1_2048_8192_SHA256
            } else {
                &RSA_PSS_2048_8192_SHA256
            };
            let verified = RsaPublicKeyComponents { n: &n, e: &e }.verify(alg, signing_input, signature);
            verified.map_err(|_| Error::IdToken)
        }
        ("ES256", "EC") => {
            if key.crv.as_deref() != Some("P-256") {
                return Err(Error::IdToken);
            }
            let (Some(x), Some(y)) = (key.x.as_ref(), key.y.as_ref()) else {
                return Err(Error::IdToken);
            };
            let x = URL_SAFE_NO_PAD.decode(x).map_err(|_| Error::IdToken)?;
            let y = URL_SAFE_NO_PAD.decode(y).map_err(|_| Error::IdToken)?;
            if x.len() != 32 || y.len() != 32 {
                return Err(Error::IdToken);
            }
            let mut point = vec![0x04];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            let verified = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point).verify(signing_input, signature);
            verified.map_err(|_| Error::IdToken)
        }
        _ => Err(Error::IdToken),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test vectors signed by fixed keys; the tokens stay valid for ten years.
    const EC_JWK: &str = r#"{"kty":"EC","crv":"P-256","kid":"test-es256","alg":"ES256","x":"L0o9jqv-iyqNtruTacH6Loz5oR1h2sqeJ_kEegbc1fc","y":"vR5bMZmnGfbGCmPmmDpjCzJWY2g70RUPJbCz-k5b7hs"}"#;
    const EC_TOKEN: &str = "eyJhbGciOiJFUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InRlc3QtZXMyNTYifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6MjEwNjkxNDAyMywiaWF0IjoxNzkxNTU0MDIzLCJzdWIiOiJ1c2VyLTEyMyIsIm5vbmNlIjoibm9uY2UtYWJjIiwiZW1haWwiOiJwZXRlcnNAZXhhbXBsZS5jb20ifQ.JAfaoH4Iu0Q-4gZ6MuABb8t9SzTDwO5nDK8ejkJsEU5RWo0yNKG2iGAyqe51zq8M_0E8QGOUE_Dyh2bK3sqNZg";
    /// A valid signature from a different key; its issuer is not `OpenAI`'s.
    const EC_BADISSUER_JWK: &str = r#"{"kty":"EC","crv":"P-256","kid":"test-es256-badissuer","alg":"ES256","x":"FTCJZB1DxFGvGM5bg_oQtnL3QqYtN8qyLhuqf2ttKFQ","y":"PseYzcCT95eODxQAX0ZFsHhP6iRAyJ2nJ-JeudxvOLU"}"#;
    const EC_BADISSUER_TOKEN: &str = "eyJhbGciOiJFUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InRlc3QtZXMyNTYtYmFkaXNzdWVyIn0.eyJpc3MiOiJodHRwczovL2F1dGguZXhhbXBsZS5pbnZhbGlkIiwiYXVkIjoib2FpYXBwX3Rlc3RfY2xpZW50IiwiZXhwIjoyMTA2OTE0MDIzLCJzdWIiOiJ1c2VyLTc4OSIsIm5vbmNlIjoibm9uY2UtYmFkaXNzIiwiZW1haWwiOiJ1c2VyQGV4YW1wbGUuY29tIiwiaWF0IjowfQ.dz6hBGSEGamAzJjK9tqI36I3BN9-RQHplAri1y-ESb94Ho4_JY3PoqCi7CNBOl6WESSAH3zcQ2kTFHEiymK3oA";
    const RSA_JWK: &str = r#"{"kty":"RSA","kid":"test-rs256","alg":"RS256","n":"ow5LDzKGK8M2q1xSbpIAAobQxxw0FTGakK1UZUtGDr0hnxgvXfa_OUEe6dBH2D_aFOfultXyW367tOvBPgWUc5XHGOI8C00QqWuStr45UJwaHP46BSoU6koYvybsSP7F82H5Iiaw53bhLJ5J8H-cxzoRMAlvPVvQLtaHyMyYc1LS_HqSvUbHrZkeG7TRgcTJvv7-6NQP3m07OuziKeX2D7y_6Pppybr2MPVPpdXr5pJ6RTuutldC8IN2SpaLlZeMqOPj-ZFIuG_E3UVdaR5gq_ghyJvucK4CMIT0KRb6xhriKCyudIxeTFPDqBg9T22uPBnv7a3xAhFZolxbIbeYUQ","e":"AQAB"}"#;
    const RSA_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InRlc3QtcnMyNTYifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6MjEwNjkxNDAyMywiaWF0IjoxNzkxNTU0MDIzLCJzdWIiOiJ1c2VyLTQ1NiIsIm5vbmNlIjoibm9uY2UtcnMiLCJlbWFpbCI6InJzYUBleGFtcGxlLmNvbSJ9.WcVaTbCECprsOvtTmt6AdsDFbz-ISg-23gVWTVIoQcI9ozY5xinBeM0Ta13CfxosEbszkQBM71SPb-bomRsWkJ4-sbDV3tL1TrjacaihqQvahLYNJF7wCPOiLMKB_S_Y5VMzC8I5zTLOCtBFqbxA8xzw8I4DgfarOI1pBZABSzoNgBFI-7jNuE3giAtGv41EMetwET7JTr-F3qYK_xl0CCHxMbc7Neqkn2pU7QCEDkFFTJ1xeptn-3dWvrsGRZb1CK540iwdnsqZ25h5HC5iPginQXI6vPj-_UrHFFFbj_FHYZ1u-88NRee4K7w_i3tJp6DOGiQvWq_5J0RDVFoYlw";

    fn jwks(json: &str) -> Jwks {
        serde_json::from_str(&format!("{{\"keys\":[{json}]}}")).expect("test jwks parses")
    }

    fn signed_claims(claims: &serde_json::Value) -> (String, Key) {
        use ring::{
            rand::SystemRandom,
            signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
        };
        let random = SystemRandom::new();
        let private = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let signer = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, private.as_ref(), &random).unwrap();
        let public = signer.public_key().as_ref();
        let key: Key = serde_json::from_value(serde_json::json!({
            "kty":"EC", "crv":"P-256", "kid":"issued-at",
            "x":URL_SAFE_NO_PAD.encode(&public[1..33]), "y":URL_SAFE_NO_PAD.encode(&public[33..])
        }))
        .unwrap();
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256","kid":"issued-at"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
        );
        let signature = signer.sign(&random, input.as_bytes()).unwrap();
        verify("ES256", &key, input.as_bytes(), signature.as_ref()).unwrap();
        (format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref())), key)
    }

    #[test]
    fn signed_tokens_require_issued_at_and_bound_future_clock_skew() {
        let now = 1_791_554_023;
        for (issued_at, accepted) in [
            (None, false),
            (Some(serde_json::Value::Null), false),
            (Some(serde_json::json!("1791554023")), false),
            (Some(serde_json::json!(1.5)), false),
            (Some(serde_json::json!(true)), false),
            (Some(serde_json::json!(u64::MAX)), false),
            (Some(serde_json::json!(now - 1)), true),
            (Some(serde_json::json!(now)), true),
            (Some(serde_json::json!(now + ISSUED_AT_LEEWAY_SECONDS)), true),
            (Some(serde_json::json!(now + ISSUED_AT_LEEWAY_SECONDS + 1)), false),
        ] {
            let mut claims = serde_json::json!({
                "iss":ISSUER, "aud":"issued-client", "sub":"synthetic-user", "nonce":"issued-nonce", "exp":now + 3600
            });
            if let Some(value) = issued_at {
                claims["iat"] = value;
            }
            let (token, key) = signed_claims(&claims);
            let result = validate_with_jwks_at(&token, "issued-client", "issued-nonce", &[key], now);
            assert_eq!(result.is_ok(), accepted);
            if !accepted {
                assert!(matches!(result, Err(Error::Malformed | Error::IdToken)));
            }
        }
    }

    #[test]
    fn es256_token_validates_to_its_identity() {
        let (subject, email) =
            validate_with_jwks(EC_TOKEN, "oaiapp_test_client", "nonce-abc", &jwks(EC_JWK).keys).unwrap();
        assert_eq!(subject, "user-123");
        assert_eq!(email.as_deref(), Some("peters@example.com"));
    }

    #[test]
    fn rs256_token_validates_to_its_identity() {
        let (subject, email) =
            validate_with_jwks(RSA_TOKEN, "oaiapp_test_client", "nonce-rs", &jwks(RSA_JWK).keys).unwrap();
        assert_eq!(subject, "user-456");
        assert_eq!(email.as_deref(), Some("rsa@example.com"));
    }

    #[test]
    fn a_single_signing_key_must_still_match_the_token_key_id() {
        let mut keys = jwks(EC_JWK).keys;
        keys[0].kid = Some("another-key".into());
        assert!(matches!(
            validate_with_jwks(EC_TOKEN, "oaiapp_test_client", "nonce-abc", &keys),
            Err(Error::IdToken)
        ));
    }

    #[test]
    fn valid_signatures_without_an_identifying_key_id_are_refused() {
        use ring::{
            rand::SystemRandom,
            signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
        };
        let random = SystemRandom::new();
        let private = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let signer = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, private.as_ref(), &random).unwrap();
        let public = signer.public_key().as_ref();
        let payload = EC_TOKEN.split('.').nth(1).unwrap();
        for kid in [None, Some(serde_json::Value::Null), Some(serde_json::json!(""))] {
            let mut header = serde_json::json!({"alg":"ES256"});
            if let Some(kid) = &kid {
                header["kid"] = kid.clone();
            }
            let mut key = serde_json::json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(&public[1..33]),"y":URL_SAFE_NO_PAD.encode(&public[33..])});
            if let Some(kid) = kid {
                key["kid"] = kid;
            }
            let key: Key = serde_json::from_value(key).unwrap();
            let input = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
                payload
            );
            let signature = signer.sign(&random, input.as_bytes()).unwrap();
            verify("ES256", &key, input.as_bytes(), signature.as_ref()).unwrap();
            let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
            assert!(matches!(
                validate_with_jwks(&token, "oaiapp_test_client", "nonce-abc", &[key]),
                Err(Error::IdToken)
            ));
        }
    }

    #[test]
    fn es256_requires_the_declared_curve_and_exact_coordinate_lengths() {
        for field in ["curve", "x", "y"] {
            let mut keys = jwks(EC_JWK).keys;
            match field {
                "curve" => keys[0].crv = Some("P-384".into()),
                "x" => keys[0].x = Some(URL_SAFE_NO_PAD.encode([0; 31])),
                _ => keys[0].y = Some(URL_SAFE_NO_PAD.encode([0; 33])),
            }
            assert!(matches!(
                validate_with_jwks(EC_TOKEN, "oaiapp_test_client", "nonce-abc", &keys),
                Err(Error::IdToken)
            ));
        }
    }

    #[test]
    fn wrong_audience_nonce_or_issuer_is_rejected() {
        let keys = jwks(EC_JWK).keys;
        assert!(matches!(
            validate_with_jwks(EC_TOKEN, "oaiapp_other_client", "nonce-abc", &keys),
            Err(Error::IdToken)
        ));
        assert!(matches!(
            validate_with_jwks(EC_TOKEN, "oaiapp_test_client", "nonce-other", &keys),
            Err(Error::IdToken)
        ));
        let bad_issuer_keys = jwks(EC_BADISSUER_JWK).keys;
        let (input, signature) = EC_BADISSUER_TOKEN.rsplit_once('.').unwrap();
        verify(
            "ES256",
            &bad_issuer_keys[0],
            input.as_bytes(),
            &URL_SAFE_NO_PAD.decode(signature).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            validate_with_jwks(
                EC_BADISSUER_TOKEN,
                "oaiapp_test_client",
                "nonce-badiss",
                &bad_issuer_keys
            ),
            Err(Error::IdToken)
        ));
    }

    #[test]
    fn signed_audience_arrays_allow_only_the_issued_client() {
        let keys = jwks(r#"{"kty":"EC","crv":"P-256","kid":"audience-fixture","x":"lce8dDTgm52PN6vkCQkhdT-gn_nvwaE-GHW-vnxE8dg","y":"HeVIrlBOZ43s3yBi356rp3edm08XzzreLbIHghju-Ws"}"#).keys;
        for (token, accepted) in [
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6ImF1ZGllbmNlLWZpeHR1cmUifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6WyJvYWlhcHBfdGVzdF9jbGllbnQiXSwiZXhwIjo0MTAyNDQ0ODAwLCJzdWIiOiJzeW50aGV0aWMtYXVkaWVuY2UtdXNlciIsIm5vbmNlIjoiYXVkaWVuY2Utbm9uY2UiLCJpYXQiOjB9.9c-lyOThLqR-wTuVcdGFEFwMTIe1IdpaAWRwn2znvBDUt1ykFYIDJcuIP-stCIsbU87s78SJOgWl659C7tgk8A",
                true,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6ImF1ZGllbmNlLWZpeHR1cmUifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6WyJvYWlhcHBfb3RoZXJfY2xpZW50Il0sImV4cCI6NDEwMjQ0NDgwMCwic3ViIjoic3ludGhldGljLWF1ZGllbmNlLXVzZXIiLCJub25jZSI6ImF1ZGllbmNlLW5vbmNlIiwiaWF0IjowfQ.DYWMPIcjaWiOgKxreZXReTLSCPWx80_qamrKdiPPXPmNQR71A6VIDoxhXXB992NUCBb6FOszkapK_DokM5u9JA",
                false,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6ImF1ZGllbmNlLWZpeHR1cmUifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6W10sImV4cCI6NDEwMjQ0NDgwMCwic3ViIjoic3ludGhldGljLWF1ZGllbmNlLXVzZXIiLCJub25jZSI6ImF1ZGllbmNlLW5vbmNlIiwiaWF0IjowfQ.J5THN4JQXW4yANMQfHM2DNkNYzBUt83jQw28GVTVL4OUOODgnjhF5Qwf6Td1QAfNQbFZ24pmXLHQmqLTNMNITA",
                false,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6ImF1ZGllbmNlLWZpeHR1cmUifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6WyJvYWlhcHBfdGVzdF9jbGllbnQiLCJ1bnRydXN0ZWQtY2xpZW50Il0sImV4cCI6NDEwMjQ0NDgwMCwic3ViIjoic3ludGhldGljLWF1ZGllbmNlLXVzZXIiLCJub25jZSI6ImF1ZGllbmNlLW5vbmNlIiwiYXpwIjoib2FpYXBwX3Rlc3RfY2xpZW50IiwiaWF0IjowfQ.x6yVQRdttaCsgR_LZeqoQ5bwb21hOOE5tLABn2TkaMTE1SZHI7udPrtOd7NOCoEsmFCJN7h1DW-eo7vxsLahzA",
                false,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6ImF1ZGllbmNlLWZpeHR1cmUifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6WyJvYWlhcHBfdGVzdF9jbGllbnQiXSwiZXhwIjo0MTAyNDQ0ODAwLCJzdWIiOiJzeW50aGV0aWMtYXVkaWVuY2UtdXNlciIsIm5vbmNlIjoiYXVkaWVuY2Utbm9uY2UiLCJhenAiOiJvYWlhcHBfb3RoZXJfY2xpZW50IiwiaWF0IjowfQ.ZyGt97CUDrgM_KwFlg0nqbMRH80cRY_atwCMYEIciTCR8p_Ehh0GlEkU5LAwAxPQWNnkC0G08ZEKKf_2stn8xQ",
                false,
            ),
        ] {
            let (input, signature) = token.rsplit_once('.').unwrap();
            verify(
                "ES256",
                &keys[0],
                input.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .unwrap();
            let result = validate_with_jwks(token, "oaiapp_test_client", "audience-nonce", &keys);
            assert_eq!(result.is_ok(), accepted);
        }
    }

    #[test]
    fn signed_tokens_must_be_within_their_validity_window() {
        let keys = jwks(r#"{"kty":"EC","crv":"P-256","kid":"time-fixture","x":"1x4MwaYqv_1CqixZWySiunR2x0vXt5T-xrAtfJmI000","y":"e6hrPXFqVrhp6VLT3kThbOYgF36up3n63KdMTavx7l0"}"#).keys;
        for (token, accepted) in [
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6InRpbWUtZml4dHVyZSJ9.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjoxLCJzdWIiOiJzeW50aGV0aWMtdGltZS11c2VyIiwibm9uY2UiOiJ0aW1lLW5vbmNlIiwiaWF0IjowfQ.-wLLr9EDQ_uFbu3g2wFWi-WrefUoBCTsONXkMaxBAfte4dfjj1dkr2Dxr4Zv8FTILN7cNlQXKYNkzymzU-Xhaw",
                true,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6InRpbWUtZml4dHVyZSJ9.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjo0MTAyNDQ0Nzk5LCJzdWIiOiJzeW50aGV0aWMtdGltZS11c2VyIiwibm9uY2UiOiJ0aW1lLW5vbmNlIiwiaWF0IjowfQ.mUioqIeaFOewWAJuZUakUQ595wQtkAJw55JG7AtRRwLnZk6iasIcoqaLK0-4kUeHMZTrKaTIWNIzgXDh5xPcJA",
                false,
            ),
            (
                "eyJhbGciOiJFUzI1NiIsImtpZCI6InRpbWUtZml4dHVyZSJ9.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6MSwibmJmIjowLCJzdWIiOiJzeW50aGV0aWMtdGltZS11c2VyIiwibm9uY2UiOiJ0aW1lLW5vbmNlIiwiaWF0IjowfQ.nyj-t5LbdpTMgjBdRyYQ3p0m9jGZf9LjiJO9tElG6S6QtgLwoljkQ7XRw7tYvoNrYK694PlZ1oDkWnjxvM8uDg",
                false,
            ),
        ] {
            let (input, signature) = token.rsplit_once('.').unwrap();
            verify(
                "ES256",
                &keys[0],
                input.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .unwrap();
            assert_eq!(
                validate_with_jwks(token, "oaiapp_test_client", "time-nonce", &keys).is_ok(),
                accepted
            );
        }
    }

    #[test]
    fn a_tampered_signature_is_rejected() {
        let mut token: Vec<u8> = EC_TOKEN.as_bytes().to_vec();
        *token.last_mut().expect("the token has a signature byte") ^= 0x01;
        let token = std::str::from_utf8(&token).unwrap();
        assert!(matches!(
            validate_with_jwks(token, "oaiapp_test_client", "nonce-abc", &jwks(EC_JWK).keys),
            Err(Error::IdToken | Error::Malformed)
        ));
    }
}
