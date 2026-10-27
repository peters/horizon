use super::super::fake::{Requests, github};
use super::*;
use rsa::{
    RsaPrivateKey,
    pkcs1::{EncodeRsaPrivateKey as _, LineEnding},
    rand_core::OsRng,
};
use serde_json::{Value, json};

/// A synthetic 2048-bit key for the fake GitHub. Each test makes its own, so the repository
/// holds no private key.
fn key() -> PrivateKey {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    let pem = key.to_pkcs1_pem(LineEnding::LF).unwrap();
    PrivateKey::from_pem(&pem).unwrap()
}

/// The installations answer for the accounts `logins`, with the app's slug.
fn installations(logins: &[(&str, u64)]) -> Value {
    Value::Array(
        logins
            .iter()
            .map(|(login, id)| json!({"id": id, "app_slug": "horizon-example", "account": {"login": login}}))
            .collect(),
    )
}

/// The token answer for a request that GitHub accepts.
fn granted() -> Value {
    json!({
        "token": "ghs_synthetic_installation",
        "expires_at": "2026-10-26T12:34:56Z",
        "permissions": {"contents": "write", "metadata": "read", "pull_requests": "write"},
        "repository_selection": "selected",
        "repositories": [{"full_name": "acme/web"}],
    })
}

/// The decoded JSON of the JWT segment at `index`.
fn segment(jwt: &str, index: usize) -> Value {
    let part = jwt.split('.').nth(index).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// The JWT of the request, without its `Bearer ` prefix.
fn jwt_of(requests: &Requests, index: usize) -> String {
    let requests = requests.lock().unwrap();
    let auth = requests[index].header("authorization").unwrap().to_owned();
    auth.strip_prefix("Bearer ").unwrap().to_owned()
}

#[test]
fn an_installation_token_is_limited_to_the_requested_repositories_and_permissions() {
    let (client, requests, task) = github(vec![
        (200, installations(&[("octo-cat", 7), ("acme", 9)])),
        (201, granted()),
    ]);
    let key = key();
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let token = client.installation_token(42, &key, &["acme/web".to_owned()]).unwrap();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    task.join().unwrap();

    // The JWT names the app, was issued a minute ago and expires in nine minutes.
    let jwt = jwt_of(&requests, 0);
    assert_eq!(segment(&jwt, 0), json!({"alg": "RS256", "typ": "JWT"}));
    let claims = segment(&jwt, 1);
    assert_eq!(claims["iss"], json!(42));
    let issued = claims["iat"].as_u64().unwrap();
    let expires = claims["exp"].as_u64().unwrap();
    assert!(
        (before.saturating_sub(60)..=after.saturating_sub(60)).contains(&issued),
        "the JWT is issued a minute before the mint: {claims}"
    );
    assert_eq!(expires - issued, 600, "the JWT lifetime stays inside GitHub's limits");
    assert_eq!(jwt.split('.').nth(2).unwrap().len(), 342, "a 256-byte RSA signature");

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0].line, "GET /app/installations?per_page=100&page=1 HTTP/1.1");
    assert_eq!(
        requests[1].line, "POST /app/installations/9/access_tokens HTTP/1.1",
        "the installation of the repository's account"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&requests[1].body).unwrap(),
        json!({
            "repositories": ["web"],
            "permissions": {"contents": "write", "metadata": "read", "pull_requests": "write"},
        }),
        "the request names the repositories by their name alone, and exactly these permissions"
    );

    assert_eq!(token.token.expose(), "ghs_synthetic_installation");
    assert_eq!(token.expires_at, UNIX_EPOCH + Duration::from_secs(1_793_018_096));
    assert_eq!(
        token.bot,
        Bot {
            login: "horizon-example[bot]".into(),
            name: "horizon-example[bot]".into(),
            email: "horizon-example[bot]@users.noreply.github.com".into(),
        }
    );
    let printed = format!("{token:?}");
    assert!(!printed.contains("ghs_"), "the token is never printed: {printed}");
    assert!(printed.contains("<redacted>"), "the token reads as a redaction");
}

#[test]
fn every_answer_without_a_usable_installation_leaves_no_token() {
    let key = key();
    // The app is installed, but not on the account of the requested repository.
    let (client, requests, task) = github(vec![(200, installations(&[("octo-cat", 7)]))]);
    assert_eq!(
        client.installation_token(42, &key, &["acme/web".to_owned()]).err(),
        Some(Error::NotInstalled)
    );
    task.join().unwrap();
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.line.starts_with("POST")),
        "no token is asked for"
    );

    // The installation does not cover the requested repository, so GitHub refuses it.
    let (client, requests, task) = github(vec![
        (200, installations(&[("acme", 9)])),
        (422, json!({"message": "Validation Failed"})),
    ]);
    assert_eq!(
        client.installation_token(42, &key, &["acme/web".to_owned()]).err(),
        Some(Error::NotCovered)
    );
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 2);

    // GitHub refuses the app's ID or private key.
    let (client, _, task) = github(vec![(401, json!({"message": "Bad credentials"}))]);
    assert_eq!(
        client.installation_token(42, &key, &["acme/web".to_owned()]).err(),
        Some(Error::AppKey)
    );
    task.join().unwrap();

    // A full page at the page cap says nothing about the installations beyond it.
    let page = json!(
        (1..=100)
            .map(|id| json!({"id": id, "app_slug": "horizon-example", "account": {"login": format!("owner{id}")}}))
            .collect::<Vec<_>>()
    );
    let (client, _, task) = github((1..=MAX_PAGES).map(|_| (200, page.clone())).collect());
    assert_eq!(
        client.installation_token(42, &key, &["acme/web".to_owned()]).err(),
        Some(Error::TooMany)
    );
    task.join().unwrap();
}

#[test]
fn an_installation_token_names_one_account_and_valid_repositories() {
    assert_eq!(account_of(&[]), Err(Error::NoRepository));
    assert_eq!(
        account_of(&["acme/web".to_owned(), "acme/api".to_owned()]),
        Ok("acme".to_owned())
    );
    assert_eq!(
        account_of(&["Acme/web".to_owned()]),
        Ok("acme".to_owned()),
        "GitHub names an account without case"
    );
    assert_eq!(
        account_of(&["acme/web".to_owned(), "octo-cat/web".to_owned()]),
        Err(Error::SeparateAccounts)
    );
    for repository in ["", "acme", "acme/", "/web", "acme/we b", "acme/../web"] {
        assert_eq!(
            account_of(&[repository.to_owned()]),
            Err(Error::InvalidRepository),
            "{repository:?} is not a repository name"
        );
    }
}

#[test]
fn only_an_unencrypted_pem_rsa_private_key_makes_a_signing_key() {
    for pem in [
        "",
        "not a key",
        "-----BEGIN RSA PRIVATE KEY-----\nsynthetic\n-----END RSA PRIVATE KEY-----\n",
        "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n",
    ] {
        assert_eq!(
            PrivateKey::from_pem(pem).err(),
            Some(Error::PrivateKey),
            "{pem:?} is not an unencrypted RSA private key"
        );
    }
    // A PEM without its end line is not a key either.
    let truncated = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n";
    assert_eq!(PrivateKey::from_pem(truncated).err(), Some(Error::PrivateKey));

    // A private key in PKCS#8 form is the other encoding GitHub gives.
    let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    let pkcs8 = rsa::pkcs8::EncodePrivateKey::to_pkcs8_pem(&key, LineEnding::LF).unwrap();
    assert!(PrivateKey::from_pem(&pkcs8).is_ok(), "PKCS#8 is a key too");
}
