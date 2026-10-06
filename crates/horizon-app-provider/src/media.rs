//! Exact owned-session evidence; provider URLs and log contents never enter public tool responses.
use crate::{Error, Result, api::BrowserStack, reconcile::Session};
use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use url::Url;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Video,
    Device,
    Crash,
    Appium,
    Network,
}
impl Kind {
    #[must_use]
    pub fn extension(self) -> &'static str {
        if matches!(self, Self::Video) { "mp4" } else { "log" }
    }
}
impl Session {
    fn media_url(&self, kind: Kind) -> Result<Url> {
        let url = if matches!(kind, Kind::Video) {
            Url::parse(self.media_field("video_url").map_err(|_| Error::MediaUnavailable)?)
                .map_err(|_| Error::MediaUnavailable)?
        } else {
            let url = Url::parse(
                self.media_field("appium_logs_url")
                    .map_err(|_| Error::MediaUnavailable)?,
            )
            .map_err(|_| Error::MediaUnavailable)?;
            let build = log_build(&url, self.media_id())?;
            let suffix = match kind {
                Kind::Device => "devicelogs",
                Kind::Crash => "crashlogs",
                Kind::Appium => "appiumlogs",
                Kind::Network => "networklogs",
                Kind::Video => return Err(Error::MediaUnavailable),
            };
            let mut target = Url::parse("https://api-cloud.browserstack.com").map_err(|_| Error::MediaUnavailable)?;
            target.set_query(None);
            target.set_path(&format!(
                "/app-automate/builds/{}/sessions/{}/{suffix}",
                build,
                self.media_id()
            ));
            target
        };
        if !safe_url(&url)
            || (matches!(kind, Kind::Video)
                && (url.host_str() != Some("app-automate.browserstack.com")
                    || url.path() != format!("/sessions/{}/video", self.media_id())))
        {
            return Err(Error::MediaUnavailable);
        }
        Ok(url)
    }
    /// # Errors
    /// A user dashboard link requires account authentication and contains no share token.
    pub fn dashboard_link(&self) -> Result<String> {
        let source = Url::parse(
            self.media_field("appium_logs_url")
                .map_err(|_| Error::MediaUnavailable)?,
        )
        .map_err(|_| Error::MediaUnavailable)?;
        let build = log_build(&source, self.media_id())?;
        Ok(format!(
            "https://app-automate.browserstack.com/builds/{build}/sessions/{}",
            self.media_id()
        ))
    }
}
fn log_build(url: &Url, session: &str) -> Result<String> {
    let parts = url.path().split('/').collect::<Vec<_>>();
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("api-cloud.browserstack.com" | "api.browserstack.com")
        )
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || parts.len() != 7
        || parts[1..3] != ["app-automate", "builds"]
        || parts[4] != "sessions"
        || parts[5] != session
        || parts[6] != "appiumlogs"
        || !(16..=128).contains(&parts[3].len())
        || !parts[3].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::MediaUnavailable);
    }
    Ok(parts[3].to_owned())
}

fn safe_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.host_str().is_some_and(|host| {
            host == "api-cloud.browserstack.com"
                || host == "app-automate.browserstack.com"
                || host.ends_with(".amazonaws.com")
                || host.ends_with(".cloudfront.net")
        })
}
impl BrowserStack {
    /// # Errors
    /// The host must supply only its exact journal-owned native session. Downloads share one 30s budget.
    pub fn media(&self, reference: &str, kind: Kind) -> Result<Vec<u8>> {
        self.media_with_timeout(reference, kind, Duration::from_secs(30))
    }
    /// # Errors
    /// Metadata and body downloads share the caller's bounded remaining lifetime.
    pub fn media_with_timeout(&self, reference: &str, kind: Kind, timeout: Duration) -> Result<Vec<u8>> {
        if timeout.is_zero() {
            return Err(Error::MediaUnavailable);
        }
        let timeout = timeout.min(Duration::from_secs(30));
        let deadline = Instant::now() + timeout;
        let session = self.session_with_timeout(reference, timeout.min(Duration::from_secs(10)))?;
        let url = session.media_url(kind)?;
        let bytes = download(url, kind, deadline, |url, authorized, remaining, maximum| {
            let request = self.agent.get(url.as_str());
            let request = if authorized {
                request.header("Authorization", self.authorization.as_str())
            } else {
                request
            };
            let mut response = request
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
                .map_err(|_| Error::MediaUnavailable)?;
            if response.status().is_redirection() {
                return Ok(Download::Redirect(
                    response
                        .headers()
                        .get("Location")
                        .and_then(|value| value.to_str().ok())
                        .ok_or(Error::MediaUnavailable)?
                        .to_owned(),
                ));
            }
            if !response.status().is_success() {
                return Err(Error::MediaUnavailable);
            }
            let bytes = response
                .body_mut()
                .with_config()
                .limit(maximum)
                .read_to_vec()
                .map_err(|_| Error::MediaUnavailable)?;
            Ok(Download::Body(bytes))
        })?;
        if matches!(kind, Kind::Video) {
            Ok(bytes)
        } else {
            self.redacted_logs(&bytes)
        }
    }
    fn redacted_logs(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let text = Zeroizing::new(
            std::str::from_utf8(bytes)
                .map_err(|_| Error::MediaUnavailable)?
                .to_owned(),
        );
        let encoded = self
            .authorization
            .strip_prefix("Basic ")
            .ok_or(Error::ProviderRejected)?;
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| Error::ProviderRejected)?,
        );
        let credential = std::str::from_utf8(&decoded).map_err(|_| Error::ProviderRejected)?;
        let (user, key) = credential.split_once(':').ok_or(Error::ProviderRejected)?;
        let mut output = String::new();
        for line in text.lines() {
            let lower = line.to_ascii_lowercase();
            let sensitive = line.contains(encoded)
                || line.contains(user)
                || line.contains(key)
                || line.contains(self.authorization.as_str())
                || [
                    "authorization",
                    "accesskey",
                    "access_key",
                    "password",
                    "token",
                    "cookie",
                    "bs://",
                    "capabilities",
                    "localkey",
                ]
                .iter()
                .any(|word| lower.contains(word))
                || ((lower.contains("http://") || lower.contains("https://"))
                    && (line.contains('?') || line.contains('@')));
            let line = if sensitive {
                "<redacted sensitive provider line>"
            } else {
                line
            };
            if output
                .len()
                .checked_add(line.len() + 1)
                .is_none_or(|size| size > 8 * 1024 * 1024)
            {
                return Err(Error::MediaUnavailable);
            }
            output.push_str(line);
            output.push('\n');
        }
        Ok(output.into_bytes())
    }
}

enum Download {
    Redirect(String),
    Body(Vec<u8>),
}
fn download(
    mut url: Url,
    kind: Kind,
    deadline: Instant,
    mut fetch: impl FnMut(&Url, bool, Duration, u64) -> Result<Download>,
) -> Result<Vec<u8>> {
    let maximum = if matches!(kind, Kind::Video) {
        64 * 1024 * 1024
    } else {
        8 * 1024 * 1024
    };
    for _ in 0..4 {
        if !safe_url(&url) {
            return Err(Error::MediaUnavailable);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::MediaUnavailable);
        }
        let authorized = matches!(
            url.host_str(),
            Some("api-cloud.browserstack.com" | "app-automate.browserstack.com")
        );
        let response = fetch(&url, authorized, remaining, maximum)?;
        if Instant::now() >= deadline {
            return Err(Error::MediaUnavailable);
        }
        match response {
            Download::Redirect(location) => {
                if !matches!(kind, Kind::Video) {
                    return Err(Error::MediaUnavailable);
                }
                url = url.join(&location).map_err(|_| Error::MediaUnavailable)?;
            }
            Download::Body(bytes) => {
                if bytes.is_empty()
                    || bytes.len() as u64 > maximum
                    || (matches!(kind, Kind::Video) && bytes.get(4..8) != Some(b"ftyp".as_slice()))
                {
                    return Err(Error::MediaUnavailable);
                }
                return Ok(bytes);
            }
        }
    }
    Err(Error::MediaUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_log_metadata_maps_only_the_exact_owned_path_to_the_fixed_api() {
        let id = "0123456789abcdef";
        for host in ["api.browserstack.com", "api-cloud.browserstack.com"] {
            let url = Url::parse(&format!(
                "https://{host}/app-automate/builds/aaaaaaaaaaaaaaaa/sessions/{id}/appiumlogs?token=private"
            ))
            .unwrap();
            assert_eq!(log_build(&url, id).unwrap(), "aaaaaaaaaaaaaaaa");
            assert!(log_build(&url, "different-session").is_err());
        }
        for url in [
            "https://user@api.browserstack.com/app-automate/builds/aaaaaaaaaaaaaaaa/sessions/0123456789abcdef/appiumlogs",
            "https://api.browserstack.com:444/app-automate/builds/aaaaaaaaaaaaaaaa/sessions/0123456789abcdef/appiumlogs",
            "https://untrusted.test/app-automate/builds/aaaaaaaaaaaaaaaa/sessions/0123456789abcdef/appiumlogs",
        ] {
            assert!(log_build(&Url::parse(url).unwrap(), id).is_err());
        }
    }
    #[test]
    fn video_redirects_drop_authorization_at_the_cdn_and_reject_untrusted_targets() {
        let url = Url::parse("https://app-automate.browserstack.com/sessions/owned/video?token=private").unwrap();
        let mut requests = Vec::new();
        let body = b"\0\0\0\x0cftypisom".to_vec();
        let bytes = download(
            url.clone(),
            Kind::Video,
            Instant::now() + Duration::from_secs(2),
            |url, auth, _, _| {
                requests.push((url.host_str().unwrap().to_owned(), auth));
                Ok(if requests.len() == 1 {
                    Download::Redirect("https://fixture.cloudfront.net/video?signature=private".into())
                } else {
                    Download::Body(body.clone())
                })
            },
        )
        .unwrap();
        assert_eq!(bytes, body);
        assert_eq!(
            requests,
            [
                ("app-automate.browserstack.com".into(), true),
                ("fixture.cloudfront.net".into(), false)
            ]
        );
        let mut calls = 0;
        assert!(
            download(
                url.clone(),
                Kind::Video,
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| {
                    calls += 1;
                    Ok(Download::Redirect("https://untrusted.test/video".into()))
                }
            )
            .is_err()
        );
        assert_eq!(calls, 1);
        calls = 0;
        assert!(
            download(
                url,
                Kind::Video,
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| {
                    calls += 1;
                    Ok(Download::Redirect("/loop".into()))
                }
            )
            .is_err()
        );
        assert_eq!(calls, 4);
    }
    #[test]
    fn late_oversized_or_invalid_downloads_never_become_evidence() {
        let url = Url::parse("https://api-cloud.browserstack.com/app-automate/owned/logs").unwrap();
        assert!(
            download(
                url.clone(),
                Kind::Device,
                Instant::now() + Duration::from_secs(2),
                |_, _, _, maximum| Ok(Download::Body(vec![0; usize::try_from(maximum).unwrap() + 1]))
            )
            .is_err()
        );
        assert!(
            download(
                url.clone(),
                Kind::Device,
                Instant::now() + Duration::from_millis(1),
                |_, _, _, _| {
                    std::thread::sleep(Duration::from_millis(5));
                    Ok(Download::Body(b"too late".to_vec()))
                }
            )
            .is_err()
        );
        assert!(
            download(
                url.clone(),
                Kind::Device,
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| Ok(Download::Redirect("https://fixture.cloudfront.net/logs".into()))
            )
            .is_err()
        );
        assert!(
            download(
                url,
                Kind::Video,
                Instant::now() + Duration::from_secs(2),
                |_, _, _, _| Ok(Download::Body(b"provider pending page".to_vec()))
            )
            .is_err()
        );
    }
    #[test]
    fn evidence_destinations_and_provider_log_redaction_are_bounded_to_trusted_policy() {
        for url in [
            "http://app-automate.browserstack.com/x",
            "https://127.0.0.1/x",
            "https://example.com/x",
            "https://user@app-automate.browserstack.com/x",
            "https://app-automate.browserstack.com:444/x",
            "https://app-automate.browserstack.com/x#fragment",
            "https://cloudfront.net.attacker.test/x",
        ] {
            assert!(!safe_url(&Url::parse(url).unwrap()));
        }
        let auth = base64::engine::general_purpose::STANDARD.encode("fixture-user:fixture-key");
        let provider = BrowserStack::new(
            "https://hub-cloud.browserstack.com",
            Zeroizing::new(format!("Basic {auth}")),
        )
        .unwrap();
        let bytes=b"Useful native stack frame\nfixture-key\nbs://aaaaaaaaaaaaaaaa\nhttps://example.test/video?token=secret\nAuthorization: hidden\nCookie: private\n";
        let result = String::from_utf8(provider.redacted_logs(bytes).unwrap()).unwrap();
        assert!(result.contains("Useful native stack frame"));
        for forbidden in ["fixture-key", "bs://", "secret", "hidden", "private"] {
            assert!(!result.contains(forbidden));
        }
    }
    #[test]
    fn redaction_output_refuses_expansion_and_obeys_the_exact_log_limit() {
        let auth = base64::engine::general_purpose::STANDARD.encode("fixture-user:fixture-key");
        let provider = BrowserStack::new(
            "https://hub-cloud.browserstack.com",
            Zeroizing::new(format!("Basic {auth}")),
        )
        .unwrap();
        let exact = vec![b'x'; 8 * 1024 * 1024 - 1];
        assert_eq!(provider.redacted_logs(&exact).unwrap().len(), 8 * 1024 * 1024);
        assert!(provider.redacted_logs(&vec![b'x'; 8 * 1024 * 1024]).is_err());
        let expansion = b"token\n".repeat(300_000);
        assert!(expansion.len() < 8 * 1024 * 1024);
        assert!(provider.redacted_logs(&expansion).is_err());
    }
}
