//! The Signature Version 4 algorithm: the canonical request, the string to
//! sign, the signing key and the `Authorization` header.
//!
//! The path is signed as the URL writes it, escape for escape, without dot
//! segment normalization, because an EC2 Query API endpoint has the path `/`.
use super::{Credential, Error, Result};
use ring::{digest, hmac};
use std::collections::BTreeMap;
use time::{OffsetDateTime, UtcOffset};
use ureq::http::Uri;
use zeroize::Zeroizing;

/// Header names that this module fills itself, so a caller cannot sign them.
const RESERVED: [&str; 3] = ["host", "x-amz-date", "authorization"];
const LOWER_HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];
const UPPER_HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
];

/// The parts of a request that enter the canonical request.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// The HTTP method, uppercase, as it is sent, such as `POST`.
    pub method: &'a str,
    /// The absolute URL that is sent.
    pub url: &'a str,
    /// The signing region, such as `eu-north-1`.
    pub region: &'a str,
    /// The signing service, such as `ec2`.
    pub service: &'a str,
    /// The exact body bytes, or an empty slice for a body that is not sent.
    pub payload: &'a [u8],
    /// Headers to sign besides `host` and `x-amz-date`, such as `content-type`.
    pub headers: &'a [(&'a str, &'a str)],
}

/// The header values that the signature depends on. Send both unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The value of the `Authorization` header.
    pub authorization: String,
    /// The value of the `X-Amz-Date` header.
    pub x_amz_date: String,
}

/// Sign one request with `AWS4-HMAC-SHA256`.
///
/// The caller pins `timestamp`, so a recorded request signs again byte for
/// byte. Nothing is fetched and nothing is read from a file.
///
/// # Errors
/// Refuses a credential with unusable characters, a URL that is not absolute or
/// holds a broken percent escape, a region or service name that does not fit a
/// scope, and a header that is malformed, repeated or one of `host`,
/// `x-amz-date` and `authorization`.
pub fn sign(request: &Request<'_>, credential: &Credential, timestamp: OffsetDateTime) -> Result<Signed> {
    if !scope_part(request.region) || !scope_part(request.service) {
        return Err(Error::Scope);
    }
    let (date, x_amz_date) = stamps(timestamp);
    let canonical = canonical_request(request, &x_amz_date)?;
    let scope = format!("{date}/{}/{}/aws4_request", request.region, request.service);
    let string_to_sign = string_to_sign(&x_amz_date, &scope, &canonical.request);
    let key = signing_key(credential, &date, request.region, request.service);
    let signature = hex(&mac(&key, string_to_sign.as_bytes()));
    Ok(Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
            credential.access_key_id(),
            canonical.signed_headers
        ),
        x_amz_date,
    })
}

/// A canonical request and the names of the headers it signs.
struct Canonical {
    request: String,
    signed_headers: String,
}

fn canonical_request(request: &Request<'_>, x_amz_date: &str) -> Result<Canonical> {
    if !uppercase(request.method) {
        return Err(Error::Request);
    }
    let uri = Uri::try_from(request.url).map_err(|_| Error::Url)?;
    if uri.scheme_str().is_none() {
        return Err(Error::Url);
    }
    let headers = canonical_headers(request.headers, &host(&uri)?, x_amz_date)?;
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        request.method,
        canonical_uri(uri.path())?,
        canonical_query(uri.query())?,
        headers.text,
        headers.names,
        hex(digest::digest(&digest::SHA256, request.payload).as_ref()),
    );
    Ok(Canonical {
        request: canonical,
        signed_headers: headers.names,
    })
}

fn string_to_sign(x_amz_date: &str, scope: &str, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{x_amz_date}\n{scope}\n{}",
        hex(digest::digest(&digest::SHA256, canonical_request.as_bytes()).as_ref())
    )
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` in UTC, with whole seconds.
fn stamps(timestamp: OffsetDateTime) -> (String, String) {
    let timestamp = timestamp.to_offset(UtcOffset::UTC);
    let date = format!(
        "{:04}{:02}{:02}",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day()
    );
    let stamp = format!(
        "{date}T{:02}{:02}{:02}Z",
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    );
    (date, stamp)
}

/// The signing key chain; every link is a secret, so each one is wiped.
fn signing_key(credential: &Credential, date: &str, region: &str, service: &str) -> Zeroizing<Vec<u8>> {
    let mut key = Zeroizing::new(format!("AWS4{}", credential.secret_access_key()).into_bytes());
    for part in [date, region, service, "aws4_request"] {
        key = mac(&key, part.as_bytes());
    }
    key
}

fn mac(key: &[u8], message: &[u8]) -> Zeroizing<Vec<u8>> {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), message);
    Zeroizing::new(tag.as_ref().to_vec())
}

/// The canonical header lines, and the names of the headers they carry.
struct Headers {
    text: String,
    names: String,
}

fn canonical_headers(extra: &[(&str, &str)], host: &str, x_amz_date: &str) -> Result<Headers> {
    let mut headers = BTreeMap::new();
    headers.insert("host".to_string(), host.to_string());
    headers.insert("x-amz-date".to_string(), x_amz_date.to_string());
    for (name, value) in extra {
        let name = name.to_ascii_lowercase();
        if !token(&name) || RESERVED.contains(&name.as_str()) || !header_value(value) {
            return Err(Error::Request);
        }
        if headers.insert(name, collapse(value)).is_some() {
            return Err(Error::Request);
        }
    }
    let names = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let mut text = String::with_capacity(headers.iter().map(|(name, value)| name.len() + value.len() + 2).sum());
    for (name, value) in &headers {
        text.push_str(name);
        text.push(':');
        text.push_str(value);
        text.push('\n');
    }
    Ok(Headers { text, names })
}

/// The `Host` value: the URI host, with a port that is not the scheme default.
fn host(uri: &Uri) -> Result<String> {
    let host = uri.host().ok_or(Error::Url)?;
    match uri.port_u16().filter(|port| !default_port(uri.scheme_str(), *port)) {
        Some(port) => Ok(format!("{host}:{port}")),
        None => Ok(host.to_string()),
    }
}

fn default_port(scheme: Option<&str>, port: u16) -> bool {
    matches!((scheme, port), (Some("http"), 80) | (Some("https"), 443))
}

/// The path as the URL encodes it, so an escape is not encoded twice.
fn canonical_uri(path: &str) -> Result<String> {
    let bytes = if path.is_empty() { b"/" } else { path.as_bytes() };
    let mut text = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            push_escape(&mut text, escape(bytes, index)?);
            index += 3;
            continue;
        }
        if unreserved(byte) || byte == b'/' {
            text.push(char::from(byte));
        } else {
            push_escape(&mut text, byte);
        }
        index += 1;
    }
    Ok(text)
}

/// Every parameter encoded and then sorted by name, and by value within a name.
fn canonical_query(query: Option<&str>) -> Result<String> {
    let Some(query) = query else {
        return Ok(String::new());
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        pairs.push((encoded(&decode(name)?), encoded(&decode(value)?)));
    }
    pairs.sort();
    Ok(pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&"))
}

/// Percent-decode, so that an escape of a non-reserved byte is encoded as one.
fn decode(text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            decoded.push(escape(bytes, index)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

/// The byte of a `%XX` escape that starts at `index`.
fn escape(bytes: &[u8], index: usize) -> Result<u8> {
    let Some([high, low]) = bytes.get(index.saturating_add(1)..index.saturating_add(3)) else {
        return Err(Error::Url);
    };
    let high = nibble(*high).ok_or(Error::Url)?;
    let low = nibble(*low).ok_or(Error::Url)?;
    Ok(high * 16 + low)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encoded(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for &byte in bytes {
        if unreserved(byte) {
            text.push(char::from(byte));
        } else {
            push_escape(&mut text, byte);
        }
    }
    text
}

fn push_escape(text: &mut String, byte: u8) {
    text.push('%');
    text.push(UPPER_HEX[usize::from(byte >> 4)]);
    text.push(UPPER_HEX[usize::from(byte & 0x0f)]);
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// An RFC 9110 token, which a method and a header name must be.
fn token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// An uppercase RFC 9110 token, which an HTTP method must be.
fn uppercase(text: &str) -> bool {
    token(text) && !text.bytes().any(|byte| byte.is_ascii_lowercase())
}

/// Printable ASCII and horizontal tabs, so no line break can join a request.
fn header_value(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
}

/// Trim the ends and fold each run of spaces and tabs into one space.
fn collapse(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut space = false;
    for byte in value.trim_matches([' ', '\t']).bytes() {
        if byte == b' ' || byte == b'\t' {
            space = true;
            continue;
        }
        if space {
            text.push(' ');
            space = false;
        }
        text.push(char::from(byte));
    }
    text
}

fn scope_part(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    text.extend(
        bytes
            .iter()
            .flat_map(|byte| [LOWER_HEX[usize::from(byte >> 4)], LOWER_HEX[usize::from(byte & 0x0f)]]),
    );
    text
}

#[cfg(test)]
mod tests {
    use super::{Request, canonical_query, canonical_request, canonical_uri, sign, stamps, string_to_sign};
    use crate::aws::{Credential, Error};
    use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, UtcOffset};

    /// The credential, the timestamp, the region and the service of the AWS
    /// Signature Version 4 test suite: `tests/aws-sig-v4-test-suite` in
    /// `awslabs/aws-c-auth` v0.3.15 (Apache-2.0, Amazon.com, Inc.), also
    /// shipped with `botocore`.
    const ACCESS_KEY: &str = "AKIDEXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const REGION: &str = "us-east-1";
    const SERVICE: &str = "service";
    const STAMP: &str = "20150830T123600Z";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn credential() -> Credential {
        Credential::new(ACCESS_KEY.to_string(), SECRET_KEY.to_string()).unwrap()
    }

    /// 2015-08-30T12:36:00Z, the timestamp of every suite case.
    fn suite_time() -> OffsetDateTime {
        Date::from_calendar_date(2015, Month::August, 30)
            .and_then(|date| date.with_hms(12, 36, 0))
            .map(PrimitiveDateTime::assume_utc)
            .unwrap()
    }

    fn suite_request<'a>(method: &'a str, headers: &'a [(&'a str, &'a str)], payload: &'a [u8]) -> Request<'a> {
        Request {
            method,
            url: "https://example.amazonaws.com/",
            region: REGION,
            service: SERVICE,
            payload,
            headers,
        }
    }

    #[test]
    fn a_get_without_a_body_reproduces_the_published_vector() {
        let signed = sign(&suite_request("GET", &[], b""), &credential(), suite_time()).unwrap();
        assert_eq!(
            signed.authorization,
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, ",
                "SignedHeaders=host;x-amz-date, ",
                "Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
            )
        );
        assert_eq!(signed.x_amz_date, STAMP);
    }

    #[test]
    fn a_post_with_a_form_body_reproduces_the_published_vector() {
        // The suite's `creq` in aws-c-auth v0.3.15 lists `content-length` and
        // `charset=utf-8` but records the hash of the variant below, which is
        // the one botocore ships and verifies. Both copies record this
        // signature.
        let request = suite_request(
            "POST",
            &[("Content-Type", "application/x-www-form-urlencoded; charset=utf8")],
            b"Param1=value1",
        );
        let signed = sign(&request, &credential(), suite_time()).unwrap();
        assert_eq!(
            signed.authorization,
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, ",
                "SignedHeaders=content-type;host;x-amz-date, ",
                "Signature=1a72ec8f64bd914b0e42e42607c7fbce7fb2c7465f63e3092b3b0d39fa77a6fe",
            )
        );
    }

    #[test]
    fn a_post_without_a_body_reproduces_the_published_vector() {
        let signed = sign(&suite_request("POST", &[], b""), &credential(), suite_time()).unwrap();
        assert_eq!(
            signed.authorization,
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, ",
                "SignedHeaders=host;x-amz-date, ",
                "Signature=5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
            )
        );
    }

    #[test]
    fn sorted_query_parameters_reproduce_the_published_vector() {
        let request = Request {
            url: "https://example.amazonaws.com/?Param2=value2&Param1=value1",
            ..suite_request("GET", &[], b"")
        };
        let signed = sign(&request, &credential(), suite_time()).unwrap();
        assert_eq!(
            signed.authorization,
            concat!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, ",
                "SignedHeaders=host;x-amz-date, ",
                "Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
            )
        );
    }

    #[test]
    fn the_canonical_request_and_the_string_to_sign_reproduce_the_published_vector() {
        let request = suite_request("GET", &[], b"");
        let canonical = canonical_request(&request, STAMP).unwrap();
        assert_eq!(
            canonical.request,
            concat!(
                "GET\n/\n\n",
                "host:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n",
                "host;x-amz-date\n",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
        );
        assert_eq!(canonical.signed_headers, "host;x-amz-date");
        assert_eq!(
            string_to_sign(STAMP, "20150830/us-east-1/service/aws4_request", &canonical.request),
            concat!(
                "AWS4-HMAC-SHA256\n20150830T123600Z\n",
                "20150830/us-east-1/service/aws4_request\n",
                "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63",
            )
        );
    }

    #[test]
    fn a_signed_header_is_lowercase_with_its_spaces_collapsed() {
        let request = suite_request("POST", &[("Content-Type", "  text/plain \t  charset=utf-8  ")], b"");
        let canonical = canonical_request(&request, STAMP).unwrap();
        assert_eq!(canonical.signed_headers, "content-type;host;x-amz-date");
        assert_eq!(
            canonical.request,
            format!(
                "POST\n/\n\ncontent-type:text/plain charset=utf-8\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\ncontent-type;host;x-amz-date\n{EMPTY_SHA256}"
            )
        );
    }

    #[test]
    fn a_path_keeps_the_escaping_the_url_gives_it() {
        assert_eq!(canonical_uri("").unwrap(), "/");
        assert_eq!(canonical_uri("/a%20b!").unwrap(), "/a%20b%21");
        assert_eq!(canonical_uri("/a%2fb").unwrap(), "/a%2Fb");
        assert_eq!(canonical_uri("/a%zz").err(), Some(Error::Url));
        assert_eq!(canonical_uri("/a%2").err(), Some(Error::Url));
    }

    #[test]
    fn every_query_parameter_is_encoded_and_sorted() {
        assert_eq!(canonical_query(None).unwrap(), "");
        assert_eq!(
            canonical_query(Some("Param2=value2&Param1=value1")).unwrap(),
            "Param1=value1&Param2=value2"
        );
        assert_eq!(canonical_query(Some("B=%2f&A=a+b")).unwrap(), "A=a%2Bb&B=%2F");
        assert_eq!(canonical_query(Some("Plain")).unwrap(), "Plain=");
        assert_eq!(canonical_query(Some("Bad=%q1")).err(), Some(Error::Url));
    }

    #[test]
    fn a_port_that_is_the_scheme_default_is_not_part_of_the_signed_host() {
        let plain = canonical_request(&suite_request("GET", &[], b""), STAMP).unwrap();
        let explicit = canonical_request(
            &Request {
                url: "https://example.amazonaws.com:443/",
                ..suite_request("GET", &[], b"")
            },
            STAMP,
        )
        .unwrap();
        let other = canonical_request(
            &Request {
                url: "https://example.amazonaws.com:8443/",
                ..suite_request("GET", &[], b"")
            },
            STAMP,
        )
        .unwrap();
        assert_eq!(plain.request, explicit.request);
        assert!(other.request.contains("\nhost:example.amazonaws.com:8443\n"));
    }

    #[test]
    fn the_stamp_is_utc_whatever_offset_the_timestamp_carries() {
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        assert_eq!(
            stamps(suite_time().to_offset(offset)),
            ("20150830".to_string(), STAMP.to_string())
        );
    }

    #[test]
    fn the_same_input_signs_the_same_header_and_another_time_does_not() {
        let request = suite_request("POST", &[], b"Action=DescribeRegions&Version=2016-11-15");
        let first = sign(&request, &credential(), suite_time()).unwrap();
        let second = sign(&request, &credential(), suite_time()).unwrap();
        assert_eq!(first, second);
        let later = sign(&request, &credential(), suite_time() + time::Duration::seconds(1)).unwrap();
        assert_ne!(first.authorization, later.authorization);
        assert_eq!(later.x_amz_date, "20150830T123601Z");
    }

    #[test]
    fn nothing_that_is_printed_carries_the_secret() {
        let signed = sign(&suite_request("POST", &[], b""), &credential(), suite_time()).unwrap();
        let prints = [
            format!("{signed:?}"),
            format!("{:?}", suite_request("POST", &[], b"")),
            format!("{:?}", credential()),
            Error::Request.to_string(),
        ];
        for print in prints {
            assert!(!print.contains(SECRET_KEY), "{print}");
            assert!(!print.contains("wJalrXUtnFEMI"), "{print}");
        }
    }

    #[test]
    fn a_request_that_cannot_be_signed_is_refused() {
        let base = Request {
            method: "POST",
            url: "https://ec2.eu-north-1.amazonaws.com/",
            region: "eu-north-1",
            service: "ec2",
            payload: b"Action=DescribeRegions&Version=2016-11-15",
            headers: &[],
        };
        let refused = |request: Request<'_>| sign(&request, &credential(), suite_time()).err();

        assert_eq!(refused(Request { url: "/ec2", ..base }), Some(Error::Url));
        assert_eq!(
            refused(Request {
                url: "https://example.amazonaws.com/%zz",
                ..base
            }),
            Some(Error::Url)
        );
        assert_eq!(refused(Request { method: "post", ..base }), Some(Error::Request));
        assert_eq!(refused(Request { region: "", ..base }), Some(Error::Scope));
        assert_eq!(
            refused(Request {
                service: "ec 2",
                ..base
            }),
            Some(Error::Scope)
        );
        assert_eq!(
            refused(Request {
                headers: &[("content-type", "text/plain\r\nX-Injected: 1")],
                ..base
            }),
            Some(Error::Request)
        );
        assert_eq!(
            refused(Request {
                headers: &[("Host", "elsewhere.example.com")],
                ..base
            }),
            Some(Error::Request)
        );
        assert_eq!(
            refused(Request {
                headers: &[("content-type", "a"), ("Content-Type", "b")],
                ..base
            }),
            Some(Error::Request)
        );
        assert!(refused(base).is_none());
    }
}
