//! ID-token validation: signature against `OpenAI`'s published JWKS, plus issuer,
//! audience, expiry and nonce checks. A token that does not validate is an error,
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
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: String,
    exp: i64,
    sub: String,
    nonce: String,
    #[serde(default)]
    email: Option<String>,
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
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Malformed);
    };
    let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| Error::Malformed)?)
        .map_err(|_| Error::Malformed)?;
    let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| Error::Malformed)?)
        .map_err(|_| Error::Malformed)?;
    let signature = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|_| Error::Malformed)?;

    if claims.iss != ISSUER {
        return Err(Error::IdToken);
    }
    if claims.aud != client_id {
        return Err(Error::IdToken);
    }
    if claims.nonce != nonce {
        return Err(Error::IdToken);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| i64::try_from(age.as_secs()).unwrap_or(0));
    if claims.exp <= now {
        return Err(Error::IdToken);
    }

    let key = keys
        .iter()
        .find(|key| key.kid.as_deref() == header.kid.as_deref())
        .or_else(|| (keys.len() == 1).then(|| &keys[0]))
        .ok_or(Error::IdToken)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    verify(&header.alg, key, signing_input.as_bytes(), &signature)?;
    Ok((claims.sub, claims.email))
}

/// GETs a JSON document, refusing non-2xx answers.
fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let response = ureq::get(url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| Error::Provider(error.to_string()))?;
    let status = response.status();
    let mut body = response.into_body();
    if !(200..300).contains(&status.as_u16()) {
        return Err(Error::Provider(format!("the sign-in service answered {status}")));
    }
    body.read_json::<T>().map_err(|_| Error::Malformed)
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
            let (Some(x), Some(y)) = (key.x.as_ref(), key.y.as_ref()) else {
                return Err(Error::IdToken);
            };
            let x = URL_SAFE_NO_PAD.decode(x).map_err(|_| Error::IdToken)?;
            let y = URL_SAFE_NO_PAD.decode(y).map_err(|_| Error::IdToken)?;
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
    const RSA_JWK: &str = r#"{"kty":"RSA","kid":"test-rs256","alg":"RS256","n":"ow5LDzKGK8M2q1xSbpIAAobQxxw0FTGakK1UZUtGDr0hnxgvXfa_OUEe6dBH2D_aFOfultXyW367tOvBPgWUc5XHGOI8C00QqWuStr45UJwaHP46BSoU6koYvybsSP7F82H5Iiaw53bhLJ5J8H-cxzoRMAlvPVvQLtaHyMyYc1LS_HqSvUbHrZkeG7TRgcTJvv7-6NQP3m07OuziKeX2D7y_6Pppybr2MPVPpdXr5pJ6RTuutldC8IN2SpaLlZeMqOPj-ZFIuG_E3UVdaR5gq_ghyJvucK4CMIT0KRb6xhriKCyudIxeTFPDqBg9T22uPBnv7a3xAhFZolxbIbeYUQ","e":"AQAB"}"#;
    const RSA_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InRlc3QtcnMyNTYifQ.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0X2NsaWVudCIsImV4cCI6MjEwNjkxNDAyMywiaWF0IjoxNzkxNTU0MDIzLCJzdWIiOiJ1c2VyLTQ1NiIsIm5vbmNlIjoibm9uY2UtcnMiLCJlbWFpbCI6InJzYUBleGFtcGxlLmNvbSJ9.WcVaTbCECprsOvtTmt6AdsDFbz-ISg-23gVWTVIoQcI9ozY5xinBeM0Ta13CfxosEbszkQBM71SPb-bomRsWkJ4-sbDV3tL1TrjacaihqQvahLYNJF7wCPOiLMKB_S_Y5VMzC8I5zTLOCtBFqbxA8xzw8I4DgfarOI1pBZABSzoNgBFI-7jNuE3giAtGv41EMetwET7JTr-F3qYK_xl0CCHxMbc7Neqkn2pU7QCEDkFFTJ1xeptn-3dWvrsGRZb1CK540iwdnsqZ25h5HC5iPginQXI6vPj-_UrHFFFbj_FHYZ1u-88NRee4K7w_i3tJp6DOGiQvWq_5J0RDVFoYlw";

    fn jwks(json: &str) -> Jwks {
        serde_json::from_str(&format!("{{\"keys\":[{json}]}}")).expect("test jwks parses")
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
