use super::*;
use crate::cloud_runtime::registry::tests::fixture;
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
};

fn token(actions: &[&str]) -> String {
    let claims = serde_json::json!({
        "aud":"example.azurecr.io", "exp":time::OffsetDateTime::now_utc().unix_timestamp()+3600,
        "access":[{"type":"repository","name":"team/worker","actions":actions}]
    });
    jwt(&claims)
}
fn jwt(claims: &serde_json::Value) -> String {
    format!(
        "header.{}.signature",
        general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

#[test]
fn authentication_without_requested_repository_actions_is_refused() {
    for (actions, pull, push) in [
        (vec![], false, false),
        (vec!["pull"], true, false),
        (vec!["push"], false, false),
        (vec!["pull", "push"], true, true),
    ] {
        let token = token(&actions);
        assert_eq!(
            check_grant(&token, "example.azurecr.io", "team/worker", Purpose::Pull).is_ok(),
            pull
        );
        assert_eq!(
            check_grant(&token, "example.azurecr.io", "team/worker", Purpose::Publish).is_ok(),
            push
        );
        assert!(check_grant(&token, "example.azurecr.io", "other/worker", Purpose::Pull).is_err());
        assert!(check_grant(&token, "other.azurecr.io", "team/worker", Purpose::Pull).is_err());
    }
}

#[test]
fn expired_future_malformed_and_wrong_resource_grants_are_refused() {
    let base = serde_json::json!({"aud":"example.azurecr.io", "exp":time::OffsetDateTime::now_utc().unix_timestamp()+3600,
        "access":[{"type":"repository", "name":"team/worker", "actions":["pull"]}]});
    for change in ["expiry", "future", "type", "empty"] {
        let mut claims = base.clone();
        match change {
            "expiry" => claims["exp"] = 0.into(),
            "future" => claims["nbf"] = (time::OffsetDateTime::now_utc().unix_timestamp() + 3600).into(),
            "type" => claims["access"][0]["type"] = "registry".into(),
            _ => claims["access"] = serde_json::json!([]),
        }
        assert!(check_grant(&jwt(&claims), "example.azurecr.io", "team/worker", Purpose::Pull).is_err());
    }
    for token in ["opaque", "header.@@.signature", "header.e30.signature", "header.e30."] {
        assert!(check_grant(token, "example.azurecr.io", "team/worker", Purpose::Pull).is_err());
    }
    for host in ["a.azurecr.io", "a.azurecr.cn", "a.azurecr.us"] {
        assert!(is_acr(host));
    }
    for host in ["azurecr.io", "a.azurecr.io.evil.test", "registry.example"] {
        assert!(!is_acr(host));
    }
}

#[test]
fn token_exchange_requests_exact_actions_and_refuses_denial_redirect_or_empty_grant() {
    let (_root, settings) = fixture();
    let binding = &settings.registries.as_ref().unwrap().bindings[0];
    let auth = binding.publish.as_ref().unwrap();
    let material = Material::load(auth, &binding.repository, None).unwrap();
    for (status, field, actions, expected) in [
        (200, "token", vec!["pull", "push"], true),
        (200, "access_token", vec!["pull", "push"], true),
        (200, "token", vec!["pull"], false),
        (200, "token", vec![], false),
        (401, "token", vec![], false),
        (403, "token", vec![], false),
        (302, "token", vec![], false),
        (503, "token", vec![], false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/oauth2/token", listener.local_addr().unwrap());
        let body = serde_json::json!({field:token(&actions)}).to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            write!(stream, "HTTP/1.1 {status} Test\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            request
        });
        let result = request(
            &endpoint,
            "example.azurecr.io",
            "team/worker",
            auth,
            &material,
            Purpose::Publish,
            &Cancellation::default(),
        );
        assert_eq!(result.is_ok(), expected);
        let recorded = server.join().unwrap();
        assert!(recorded.starts_with(
            "GET /oauth2/token?service=example.azurecr.io&scope=repository%3Ateam%2Fworker%3Apull%2Cpush "
        ));
        assert!(recorded.to_lowercase().contains("authorization: basic "));
        if let Err(error) = result {
            assert!(!error.to_string().contains("synthetic"));
        }
    }
}

#[test]
fn cancellation_prevents_network_contact() {
    let (_root, settings) = fixture();
    let binding = &settings.registries.as_ref().unwrap().bindings[0];
    let auth = &binding.pull;
    let material = Material::load(auth, &binding.repository, None).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    for repository in [
        "EXAMPLE.AZURECR.IO/team/worker",
        "example.azurecr.io:443/team/worker",
        "example.azurecr.io:0443/team/worker",
    ] {
        assert!(matches!(
            verify(auth, &material, repository, Purpose::Pull, &cancel),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
    }
    assert!(matches!(
        request(
            "http://127.0.0.1:1",
            "example.azurecr.io",
            "team/worker",
            auth,
            &material,
            Purpose::Pull,
            &cancel
        ),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
}
