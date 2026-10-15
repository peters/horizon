use super::RetentionReason;
use std::{
    fmt::Write,
    io::Read,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const ORIGIN: &str = "https://api.tailscale.com/api/v2";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const BODY_LIMIT: usize = 16 * 1024;

pub(super) enum Request<'a> {
    OAuth { client_id: &'a str, client_secret: &'a str },
    Delete { device_id: &'a str, bearer: &'a str },
}
pub(super) struct Response {
    pub status: u16,
    pub body: Zeroizing<Vec<u8>>,
}
pub(super) trait Transport {
    fn send(&self, request: Request<'_>, deadline: Instant) -> Result<Response, RetentionReason>;
}
pub(super) struct HttpTransport {
    agent: ureq::Agent,
    origin: String,
}
impl HttpTransport {
    pub fn new() -> Self {
        Self::at(ORIGIN.into(), true)
    }
    fn at(origin: String, https: bool) -> Self {
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .https_only(https)
            .proxy(None)
            .max_response_header_size(BODY_LIMIT)
            .http_status_as_error(false)
            .timeout_global(Some(HTTP_TIMEOUT))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            origin,
        }
    }
    #[cfg(test)]
    pub(super) fn for_test(origin: String) -> Self {
        Self::at(origin, false)
    }
}

pub(super) fn device_url(id: &str) -> Result<String, RetentionReason> {
    // Encode a single opaque segment, including dots, slashes, %, ? and #.
    if id == "." || id == ".." || id.is_empty() || id.len() > 256 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(RetentionReason::UnexpectedResponse);
    }
    let mut url = format!("{ORIGIN}/device/");
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'~' {
            url.push(char::from(byte));
        } else {
            write!(url, "%{byte:02X}").map_err(|_| RetentionReason::UnexpectedResponse)?;
        }
    }
    Ok(url)
}

impl Transport for HttpTransport {
    fn send(&self, request: Request<'_>, deadline: Instant) -> Result<Response, RetentionReason> {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(RetentionReason::Transport)?
            .min(HTTP_TIMEOUT);
        let response = match request {
            Request::OAuth {
                client_id,
                client_secret,
            } => self
                .agent
                .post(format!("{}/oauth/token", self.origin))
                .config()
                .timeout_global(Some(timeout))
                .build()
                .send_form([
                    ("grant_type", "client_credentials"),
                    ("client_id", client_id),
                    ("client_secret", client_secret),
                    ("scope", "devices:core"),
                    ("tags", "tag:horizon-worker"),
                ]),
            Request::Delete { device_id, bearer } => {
                let authorization = Zeroizing::new(format!("Bearer {bearer}"));
                let url = device_url(device_id)?;
                let path = url.strip_prefix(ORIGIN).ok_or(RetentionReason::UnexpectedResponse)?;
                self.agent
                    .delete(format!("{}{path}", self.origin))
                    .config()
                    .timeout_global(Some(timeout))
                    .build()
                    .header("Authorization", authorization.as_str())
                    .call()
            }
        }
        .map_err(|_| RetentionReason::Transport)?;
        bounded(response.status().as_u16(), response.into_body().as_reader())
    }
}

pub(super) fn bounded(status: u16, reader: impl Read) -> Result<Response, RetentionReason> {
    let mut body = Zeroizing::new(Vec::with_capacity(BODY_LIMIT + 1));
    reader
        .take((BODY_LIMIT + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| RetentionReason::Transport)?;
    if body.len() > BODY_LIMIT {
        return Err(RetentionReason::UnexpectedResponse);
    }
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_origin_encodes_every_path_control_and_refuses_unbounded_ids() {
        assert_eq!(
            device_url("n/a%?x#.."),
            Ok(format!("{ORIGIN}/device/n%2Fa%25%3Fx%23%2E%2E"))
        );
        for id in ["", ".", "..", "a b", "\r\n", &"x".repeat(257)] {
            assert!(device_url(id).is_err());
        }
        let client = HttpTransport::new();
        assert_eq!(client.agent.config().max_redirects(), 0);
        assert!(client.agent.config().https_only());
        assert_eq!(client.agent.config().max_response_header_size(), BODY_LIMIT);
        assert_eq!(client.agent.config().timeouts().global, Some(Duration::from_secs(15)));
    }
    #[test]
    fn bounds_apply_before_json_or_success_classification() {
        assert!(bounded(200, &vec![b'x'; BODY_LIMIT][..]).is_ok());
        assert!(matches!(
            bounded(200, &vec![b'x'; BODY_LIMIT + 1][..]),
            Err(RetentionReason::UnexpectedResponse)
        ));
    }
}
