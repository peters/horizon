//! Refresh timing, provider response and atomic token replacement under one session lock.
use super::{Duration, Error, Path, RESOURCE, Result, TOKEN_URL, TokenResponse, store};

pub(super) fn refresh(root: &Path, client_id: &str) -> Result<()> {
    refresh_with(root, client_id, request)
}

fn refresh_with(root: &Path, client_id: &str, request: impl FnOnce(&str, &str) -> Result<TokenResponse>) -> Result<()> {
    let lock = store::session_lock(root)?;
    let record = store::registration(&lock, client_id)?.ok_or(Error::Missing)?;
    // A request before the provider's earliest time can invalidate the rotating grant.
    if record.earliest_refresh_at.is_some_and(|time| time > store::now_unix()) {
        return Err(Error::Invalid("the session is not refreshable yet"));
    }
    let refresh_token = record
        .refresh_token
        .as_ref()
        .filter(|token| !token.trim().is_empty())
        .ok_or(Error::Invalid("The connection has no refresh token"))?;
    let token = request(client_id, refresh_token)?;
    let rotated = token.refresh_token.as_ref().ok_or_else(|| {
        Error::Provider("ChatGPT rotated the session without a new refresh token. Sign in again.".into())
    })?;
    // Check here too: test transports and future callers must obey the same boundary.
    if token.access_token.trim().is_empty() || rotated.trim().is_empty() {
        return Err(Error::Malformed);
    }
    store::replace_tokens(
        &lock,
        client_id,
        &token.access_token,
        rotated,
        token.expires_in.unwrap_or(3600),
        token.earliest_refresh_at,
        token.scope.as_deref().map_or_else(
            || record.scopes.clone(),
            |scope| scope.split_whitespace().map(str::to_owned).collect(),
        ),
    )
}

fn request(client_id: &str, refresh_token: &str) -> Result<TokenResponse> {
    let response = ureq::post(TOKEN_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form([
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("resource", RESOURCE),
        ])
        .map_err(|error| Error::Provider(error.to_string()))?;
    let status = response.status();
    if !(200..300).contains(&status.as_u16()) {
        return Err(Error::Provider(format!("the sign-in service answered {status}")));
    }
    super::super::response::read(response.into_body(), "token refresh")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(root: &Path, earliest: Option<i64>) -> std::path::PathBuf {
        store::save(
            root,
            &store::Record {
                email: Some("user@example.com".into()),
                issuer: "https://auth.openai.com".into(),
                subject: "synthetic-account".into(),
                client_id: "client-a".into(),
                ext_agent_host_id: "synthetic-host".into(),
                id_token: zeroize::Zeroizing::new("old-id".into()),
                access_token: Some(zeroize::Zeroizing::new("old-access".into())),
                refresh_token: Some(zeroize::Zeroizing::new("old-refresh".into())),
                token_type: Some("Bearer".into()),
                expires_in: Some(3600),
                earliest_refresh_at: earliest,
                scopes: vec!["openid".into(), "chatgpt.tokens.use.direct".into()],
                usage_confirmed: true,
                saved_at_unix: 1,
            },
        )
        .unwrap();
        store::set_active(root, "client-a").unwrap();
        root.join("chatgpt/client-a.json")
    }

    fn response(access: &str, refresh: Option<&str>, scope: Option<&str>) -> TokenResponse {
        TokenResponse {
            access_token: zeroize::Zeroizing::new(access.into()),
            refresh_token: refresh.map(|value| zeroize::Zeroizing::new(value.into())),
            id_token: None,
            token_type: "Bearer".into(),
            expires_in: Some(7200),
            earliest_refresh_at: Some(store::now_unix() + 600),
            scope: scope.map(str::to_owned),
        }
    }

    #[test]
    fn the_earliest_refresh_gate_never_calls_the_provider_or_changes_credentials() {
        let root = tempfile::tempdir().unwrap();
        let path = saved(root.path(), Some(store::now_unix() + 3600));
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            refresh_with(root.path(), "client-a", |_, _| {
                panic!("a premature refresh must not reach the provider")
            }),
            Err(Error::Invalid(_))
        ));
        // The public operation shares the same pre-request gate.
        assert!(matches!(
            super::super::refresh(root.path(), "client-a"),
            Err(Error::Invalid(_))
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn a_successful_refresh_rotates_both_tokens_and_preserves_the_account_and_notice() {
        for scope in [None, Some("openid")] {
            let root = tempfile::tempdir().unwrap();
            saved(root.path(), Some(0));
            refresh_with(root.path(), "client-a", |client, token| {
                assert_eq!(client, "client-a");
                assert_eq!(token, "old-refresh");
                assert!(store::session_lock(root.path()).is_err());
                Ok(response("new-access", Some("new-refresh"), scope))
            })
            .unwrap();
            let stored = store::registration(&store::session_lock(root.path()).unwrap(), "client-a")
                .unwrap()
                .unwrap();
            assert_eq!(stored.access_token.as_deref().unwrap().as_str(), "new-access");
            assert_eq!(stored.refresh_token.as_deref().unwrap().as_str(), "new-refresh");
            assert_eq!(stored.id_token.as_str(), "old-id");
            assert_eq!(stored.subject, "synthetic-account");
            assert!(stored.usage_confirmed);
            assert_eq!(stored.expires_in, Some(7200));
            assert!(stored.earliest_refresh_at.unwrap() > store::now_unix());
            assert_eq!(
                super::super::super::status(root.path()).unwrap().unwrap().plan_usage,
                scope.is_none()
            );
        }
    }

    #[test]
    fn provider_and_malformed_refresh_failures_preserve_the_current_record_byte_for_byte() {
        for kind in ["provider", "missing", "empty-access", "empty-refresh", "malformed"] {
            let root = tempfile::tempdir().unwrap();
            let path = saved(root.path(), None);
            let before = std::fs::read(&path).unwrap();
            let active = std::fs::read(root.path().join("chatgpt/active")).unwrap();
            assert!(
                refresh_with(root.path(), "client-a", |_, _| match kind {
                    "provider" => Err(Error::Provider("synthetic refusal".into())),
                    "missing" => Ok(response("new-access", None, None)),
                    "empty-access" => Ok(response("", Some("new-refresh"), None)),
                    "empty-refresh" => Ok(response("new-access", Some(""), None)),
                    _ => super::super::super::response::parse(b"{}", "synthetic refresh"),
                })
                .is_err()
            );
            assert_eq!(std::fs::read(path).unwrap(), before, "{kind}");
            assert_eq!(std::fs::read(root.path().join("chatgpt/active")).unwrap(), active);
        }
    }
}
