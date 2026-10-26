//! Installation tokens of this machine's GitHub App: a JWT that signs as the app, the
//! installation of the app that the JWT resolves, and the short-lived token GitHub mints
//! for that installation.
//!
//! The app's private key stays on this computer. The token reaches only the repositories
//! named in the request, with contents and pull requests at write access and metadata at
//! read access, and GitHub ends it after one hour. It has no refresh token, so renewal
//! needs the key and stays on this computer: a holder of the token cannot renew it.
//!
//! The bot login is `<slug>[bot]`. GitHub attributes a commit to the bot account only when
//! the author address holds the bot's numeric user ID, which neither answer of the two
//! requests below gives, so this name and address name the bot without attributing to it.
use super::{
    Client, Error, Result, Secret,
    api::{bearer, valid_login},
    read_limited, valid_slug,
};
use base64::Engine as _;
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

#[cfg(test)]
mod tests;

/// The most pages of 100 installations read. A full page at the cap is an error, never a
/// partial list that could miss the installation a repository belongs to.
const MAX_PAGES: u32 = 20;
/// The largest page of 100 full installation objects read, in bytes.
const PAGE_LIMIT: u64 = 1024 * 1024;
/// The largest private key PEM read, as for the other private files of settings.
const MAX_PEM: usize = 4096;
/// GitHub accepts a JWT that was issued at most 60 seconds ago, and one that expires at
/// most 10 minutes from now. The lifetime below stays inside both limits.
const ISSUED_BEFORE: u64 = 60;
const LIFETIME: u64 = 540;
/// What a token is asked for: enough to commit and to open a pull request, and nothing
/// else. GitHub narrows this to the permissions its app holds.
const PERMISSIONS: [(&str, &str); 3] = [("contents", "write"), ("pull_requests", "write"), ("metadata", "read")];
/// The only algorithm GitHub accepts for an app JWT.
const HEADER: &[u8] = br#"{"alg":"RS256","typ":"JWT"}"#;

/// The app's bot account: what a commit and a token of the app act as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bot {
    /// `<slug>[bot]`, the login of the bot.
    pub login: String,
    /// The author name of a commit by the app.
    pub name: String,
    /// The author address of a commit by the app.
    pub email: String,
}

/// A token of one installation of the app, and what it acts as.
#[derive(Debug)]
pub struct InstallationToken {
    /// The token value. It is wiped on drop and prints as `<redacted>`.
    pub token: Secret,
    /// When GitHub ends the token, at most one hour after the mint.
    pub expires_at: SystemTime,
    pub bot: Bot,
}

/// The app's RSA signing key, parsed from its PEM private key.
pub struct PrivateKey {
    key: RsaKeyPair,
    random: SystemRandom,
}

impl PrivateKey {
    /// Parses an unencrypted RSA private key in PEM form: `BEGIN RSA PRIVATE KEY`, which
    /// GitHub's download of an app key gives, or `BEGIN PRIVATE KEY`.
    /// # Errors
    /// [`Error::PrivateKey`] when the text is not one of those keys, or holds a key that
    /// `ring` refuses.
    pub fn from_pem(pem: &str) -> Result<Self> {
        let (encoding, der) = pem_der(pem)?;
        let key = match encoding {
            Encoding::Pkcs1 => RsaKeyPair::from_der(&der),
            Encoding::Pkcs8 => RsaKeyPair::from_pkcs8(&der),
        }
        .map_err(|_| Error::PrivateKey)?;
        Ok(Self {
            key,
            random: SystemRandom::new(),
        })
    }

    /// Signs `message` with RS256, the algorithm of an app JWT.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(&RSA_PKCS1_SHA256, &self.random, message, &mut signature)
            .map_err(|_| Error::PrivateKey)?;
        Ok(signature)
    }
}

impl Client {
    /// Mints a token of the installation that covers `repositories`, each a lowercase
    /// `owner/name` of one account.
    /// # Errors
    /// [`Error::NoRepository`] for an empty list, [`Error::SeparateAccounts`] when the
    /// repositories belong to more than one account, [`Error::NotInstalled`] when the app
    /// is not installed on their account, [`Error::NotCovered`] when the installation does
    /// not reach one of them, [`Error::AppKey`] when GitHub refuses the app's ID or private
    /// key, [`Error::TooMany`] when the app has more installations than Horizon reads, and
    /// [`Error::PrivateKey`] or [`Error::Clock`] when the key or this computer's clock
    /// cannot make the JWT.
    pub fn installation_token(
        &self,
        app_id: u64,
        key: &PrivateKey,
        repositories: &[String],
    ) -> Result<InstallationToken> {
        let account = account_of(repositories)?;
        // The JWT is a credential for the app: it is wiped and never printed.
        let jwt = jwt(app_id, key, SystemTime::now())?;
        let Some(installation) = self.installation(&jwt, &account)? else {
            return Err(Error::NotInstalled);
        };
        let body = serde_json::json!({
            "repositories": repositories,
            "permissions": PERMISSIONS.iter().copied().collect::<BTreeMap<_, _>>(),
        })
        .to_string();
        let response = self
            .agent
            .post(format!(
                "{}/app/installations/{}/access_tokens",
                self.api, installation.id
            ))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", bearer(&jwt).as_str())
            .header("Content-Type", "application/json")
            .send(body.as_bytes());
        let answer = read_limited(response, super::RESPONSE_LIMIT)?;
        let minted: Minted = match answer.status {
            201 => answer.parse()?,
            401 => return Err(Error::AppKey),
            // GitHub refuses a repository that this installation does not cover.
            422 => return Err(Error::NotCovered),
            status => return Err(Error::Refused(format!("HTTP {status}"))),
        };
        let token = minted.token;
        // A wider grant than the request is a malformed answer, never a token to keep.
        if minted.permissions.iter().any(|(name, access)| {
            PERMISSIONS
                .iter()
                .find(|(asked, _)| asked == name)
                .is_none_or(|(_, asked)| asked != access)
        }) {
            return Err(Error::InvalidResponse);
        }
        if !valid_token(&token) {
            return Err(Error::InvalidResponse);
        }
        Ok(InstallationToken {
            token,
            expires_at: expiry(&minted.expires_at)?,
            bot: bot(&installation.slug),
        })
    }

    /// The installation of the app on `account`, read with `jwt`, or `None` when the app
    /// is not installed there.
    fn installation(&self, jwt: &str, account: &str) -> Result<Option<Installation>> {
        #[derive(Deserialize)]
        struct Fields {
            id: u64,
            app_slug: String,
            account: Account,
        }
        #[derive(Deserialize)]
        struct Account {
            login: String,
        }
        for page in 1..=MAX_PAGES {
            let path = format!("/app/installations?per_page=100&page={page}");
            let response = self
                .agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", bearer(jwt).as_str())
                .call();
            let answer = read_limited(response, PAGE_LIMIT)?;
            let fields: Vec<Fields> = match answer.status {
                200 => answer.parse()?,
                401 => return Err(Error::AppKey),
                status => return Err(Error::Refused(format!("HTTP {status}"))),
            };
            let last = fields.len() < 100;
            for field in fields {
                if field.id == 0 || !valid_login(&field.account.login) || !valid_slug(&field.app_slug) {
                    return Err(Error::InvalidResponse);
                }
                if field.account.login.eq_ignore_ascii_case(account) {
                    return Ok(Some(Installation {
                        id: field.id,
                        slug: field.app_slug,
                    }));
                }
            }
            if last {
                return Ok(None);
            }
        }
        Err(Error::TooMany)
    }
}

/// A token that GitHub minted, which carries no refresh token: renewal needs the app's
/// key.
#[derive(Deserialize)]
struct Minted {
    token: Secret,
    expires_at: String,
    /// What GitHub granted. An answer without this map reports no grant, so it is never
    /// read as a wider one.
    #[serde(default)]
    permissions: BTreeMap<String, String>,
}

/// The installation of the app on one account.
struct Installation {
    id: u64,
    slug: String,
}

/// Which of the two encodings GitHub gives an app's private key in.
enum Encoding {
    /// `BEGIN RSA PRIVATE KEY`.
    Pkcs1,
    /// `BEGIN PRIVATE KEY`.
    Pkcs8,
}

/// The DER bytes of a PEM private key, in a buffer that is wiped on drop.
fn pem_der(pem: &str) -> Result<(Encoding, Zeroizing<Vec<u8>>)> {
    if pem.len() > MAX_PEM {
        return Err(Error::PrivateKey);
    }
    let (encoding, label) = if pem.contains("-----BEGIN RSA PRIVATE KEY-----") {
        (Encoding::Pkcs1, "RSA PRIVATE KEY")
    } else if pem.contains("-----BEGIN PRIVATE KEY-----") {
        (Encoding::Pkcs8, "PRIVATE KEY")
    } else {
        return Err(Error::PrivateKey);
    };
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body = pem
        .split_once(&begin)
        .and_then(|(_, rest)| rest.split_once(&end))
        .map(|(body, _)| body)
        .ok_or(Error::PrivateKey)?;
    // Line breaks and spaces are not part of the base64 text; the body moves straight into
    // a wiped buffer.
    let mut text = Zeroizing::new(String::with_capacity(body.len()));
    text.extend(body.chars().filter(|character| !character.is_ascii_whitespace()));
    let der = base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|_| Error::PrivateKey)?;
    Ok((encoding, Zeroizing::new(der)))
}

/// The JWT that authenticates as the app. GitHub accepts it for 10 minutes, and it may be
/// issued up to a minute before now.
fn jwt(app_id: u64, key: &PrivateKey, now: SystemTime) -> Result<Zeroizing<String>> {
    let seconds = now.duration_since(UNIX_EPOCH).map_err(|_| Error::Clock)?.as_secs();
    // The claims hold no secret; the signed JWT that holds them does.
    let claims = format!(
        r#"{{"iat":{},"exp":{},"iss":{app_id}}}"#,
        seconds.saturating_sub(ISSUED_BEFORE),
        seconds.saturating_add(LIFETIME)
    );
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(HEADER);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims);
    // Sized for the whole JWT up front, so no reallocation leaves a copy of it behind.
    let mut token = Zeroizing::new(String::with_capacity(header.len() + payload.len() + 400));
    token.push_str(&header);
    token.push('.');
    token.push_str(&payload);
    let signature = key.sign(token.as_bytes())?;
    token.push('.');
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode_string(signature, &mut token);
    Ok(token)
}

/// The one account that `repositories` belong to, in lowercase.
fn account_of(repositories: &[String]) -> Result<String> {
    let Some(first) = repositories.first() else {
        return Err(Error::NoRepository);
    };
    if repositories
        .iter()
        .any(|repository| !super::valid_repository(repository))
    {
        return Err(Error::InvalidRepository);
    }
    let Some((account, _)) = first.split_once('/') else {
        return Err(Error::InvalidRepository);
    };
    // One token belongs to one installation, and GitHub installs an app on each account
    // separately.
    if repositories.iter().any(|repository| {
        repository
            .split_once('/')
            .is_none_or(|(owner, _)| !owner.eq_ignore_ascii_case(account))
    }) {
        return Err(Error::SeparateAccounts);
    }
    Ok(account.to_ascii_lowercase())
}

/// The bot of the app whose slug is `slug`.
fn bot(slug: &str) -> Bot {
    let login = format!("{slug}[bot]");
    Bot {
        email: format!("{login}@users.noreply.github.com"),
        name: login.clone(),
        login,
    }
}

/// The answer's `expires_at`, an RFC 3339 time.
fn expiry(text: &str) -> Result<SystemTime> {
    let seconds = OffsetDateTime::parse(text, &Rfc3339)
        .map_err(|_| Error::InvalidResponse)?
        .unix_timestamp();
    UNIX_EPOCH
        .checked_add(Duration::from_secs(
            u64::try_from(seconds).map_err(|_| Error::InvalidResponse)?,
        ))
        .ok_or(Error::InvalidResponse)
}

/// A token value: visible ASCII, as GitHub sends it in an `Authorization` header.
fn valid_token(token: &Secret) -> bool {
    let token = token.expose();
    (1..=2048).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic())
}
