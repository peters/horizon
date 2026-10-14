//! REST calls made with a user access token: who the token acts as, and which
//! repositories the app may reach for that user. A repository's size, and whether a
//! public app has a slug, can also be read without one.
use super::{Client, Error, Result, Secret, read};
use serde::Deserialize;
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// The most pages read of installations, and of each installation's repositories; 100
/// entries each. Beyond it the answer is an error, never partial data.
const MAX_PAGES: u32 = 20;

/// The largest page of 100 full repository objects read, in bytes.
const PAGE_LIMIT: u64 = 4 * 1024 * 1024;

/// The largest search page read, in bytes: 50 pull requests with their whole descriptions,
/// which reach 65,536 characters each when Dependabot quotes release notes. A character
/// takes at most six bytes in the JSON answer (`\u0001`), so a page stays under 20 MiB.
const SEARCH_LIMIT: u64 = 24 * 1024 * 1024;

/// The account a token acts as. `email` is GitHub's private commit address, so a
/// commit never shows the person's own address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: u64,
    pub login: String,
    pub name: String,
    pub email: String,
}

impl Client {
    /// The account `token` acts as.
    /// # Errors
    /// Transport failures, a refused token, or a malformed answer.
    pub fn user(&self, token: &Secret) -> Result<User> {
        #[derive(Deserialize)]
        struct Fields {
            id: u64,
            login: String,
            #[serde(default)]
            name: Option<String>,
        }
        let fields: Fields = self.get(token, "/user")?;
        if fields.id == 0 || !valid_login(&fields.login) {
            return Err(Error::InvalidResponse);
        }
        let name = fields
            .name
            .filter(|name| !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control))
            .unwrap_or_else(|| fields.login.clone());
        Ok(User {
            id: fields.id,
            email: format!("{}+{}@users.noreply.github.com", fields.id, fields.login),
            login: fields.login,
            name,
        })
    }

    /// Every `owner/name` the app is installed on and `token`'s user can reach,
    /// in lowercase.
    /// # Errors
    /// Transport failures, a refused token, or a malformed answer.
    pub fn installed_repositories(&self, token: &Secret) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Installations {
            installations: Vec<Installation>,
        }
        #[derive(Deserialize)]
        struct Installation {
            id: u64,
        }
        #[derive(Deserialize)]
        struct Repositories {
            total_count: u64,
            repositories: Vec<Repository>,
        }
        #[derive(Deserialize)]
        struct Repository {
            full_name: String,
        }
        let mut ids = Vec::new();
        for page in 1..=MAX_PAGES {
            let answer: Installations =
                self.get_large(token, &format!("/user/installations?per_page=100&page={page}"))?;
            let last = answer.installations.len() < 100;
            ids.extend(answer.installations.into_iter().map(|installation| installation.id));
            if last {
                break;
            }
            if page == MAX_PAGES {
                return Err(Error::TooMany);
            }
        }
        let mut names = Vec::new();
        for id in ids {
            let mut read = 0_u64;
            for page in 1..=MAX_PAGES {
                let path = format!("/user/installations/{id}/repositories?per_page=100&page={page}");
                let answer: Repositories = self.get_large(token, &path)?;
                let last = answer.repositories.len() < 100;
                read += answer.repositories.len() as u64;
                for repository in answer.repositories {
                    if !valid_repository(&repository.full_name) {
                        return Err(Error::InvalidResponse);
                    }
                    names.push(repository.full_name.to_ascii_lowercase());
                }
                if last || read >= answer.total_count {
                    break;
                }
                if page == MAX_PAGES {
                    // Partial data would make a reachable repository look not installed.
                    return Err(Error::TooMany);
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The size of `repository` (`owner/name`) as GitHub counts it, in bytes: about what a
    /// full clone receives. Read with `token` when given, anonymously otherwise; GitHub
    /// recounts it about once an hour.
    /// # Errors
    /// An invalid name, transport failures, a refused token, a repository GitHub does not
    /// show, or a malformed answer.
    pub fn repository_size(&self, repository: &str, token: Option<&Secret>) -> Result<u64> {
        #[derive(Deserialize)]
        struct Fields {
            /// In KiB.
            size: u64,
        }
        if !valid_repository(repository) {
            return Err(Error::InvalidResponse);
        }
        let fields: Fields = self.request(token, &format!("/repos/{repository}"))?;
        fields.size.checked_mul(1024).ok_or(Error::InvalidResponse)
    }

    /// Whether a public app has the slug `slug`. GitHub shows no private app here, so
    /// `false` does not prove that the name is free.
    /// # Errors
    /// An invalid slug, transport failures, or an answer other than found or not found.
    pub fn app_exists(&self, slug: &str) -> Result<bool> {
        #[derive(Deserialize)]
        struct Fields {}
        if !super::valid_slug(slug) {
            return Err(Error::InvalidResponse);
        }
        match self.request::<Fields>(None, &format!("/apps/{slug}")) {
            Ok(Fields {}) => Ok(true),
            Err(Error::Refused(status)) if status == "HTTP 404" => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Whether the organization `owner` keeps `token` from its container package `name`
    /// because it limits OAuth apps; GitHub's packages API says so in a refusal. Only
    /// organizations limit OAuth apps, so a personal account's package is never limited.
    /// # Errors
    /// An invalid owner or name, transport failures, a revoked token, or an answer that
    /// says neither (such as a rate limit or a server error).
    pub fn oauth_restricted(&self, token: &Secret, owner: &str, name: &str) -> Result<bool> {
        if !valid_login(owner) || !valid_package(name) {
            return Err(Error::InvalidResponse);
        }
        let path = format!("/orgs/{owner}/packages/container/{}", name.replace('/', "%2F"));
        let answer = read(
            self.agent
                .get(format!("{}{path}", self.api))
                .header("Accept", "application/vnd.github+json")
                .header("Authorization", bearer(token).as_str())
                .call(),
        )?;
        match answer.status {
            401 => Err(Error::Revoked),
            403 if String::from_utf8_lossy(&answer.body).contains(OAUTH_RESTRICTED) => Ok(true),
            // The package is readable, or not one this login may see: no restriction says so.
            200..=299 | 403 | 404 => Ok(false),
            status => Err(Error::Refused(format!("HTTP {status}"))),
        }
    }

    fn get<T: serde::de::DeserializeOwned>(&self, token: &Secret, path: &str) -> Result<T> {
        self.request(Some(token), path)
    }

    /// As `get`, for a page of 100 full repository objects.
    fn get_large<T: serde::de::DeserializeOwned>(&self, token: &Secret, path: &str) -> Result<T> {
        self.request_limited(Some(token), path, PAGE_LIMIT)
    }

    /// As `get`, for a search page whose items carry whole pull request descriptions.
    pub(super) fn get_search<T: serde::de::DeserializeOwned>(&self, token: &Secret, path: &str) -> Result<T> {
        self.request_limited(Some(token), path, SEARCH_LIMIT)
    }

    fn request<T: serde::de::DeserializeOwned>(&self, token: Option<&Secret>, path: &str) -> Result<T> {
        self.request_limited(token, path, super::RESPONSE_LIMIT)
    }

    fn request_limited<T: serde::de::DeserializeOwned>(
        &self,
        token: Option<&Secret>,
        path: &str,
        limit: u64,
    ) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json");
        let auth;
        if let Some(token) = token {
            auth = bearer(token);
            request = request.header("Authorization", auth.as_str());
        }
        let response = request.call();
        let answer = super::read_limited(response, limit)?;
        match answer.status {
            200 => answer.parse(),
            401 => Err(Error::Revoked),
            status => Err(Error::Refused(format!("HTTP {status}"))),
        }
    }
}

/// What GitHub writes when an organization's OAuth app restrictions refuse a token.
const OAUTH_RESTRICTED: &str = "organization has enabled OAuth App access restrictions";

/// The `Authorization` value for `token`, in a buffer that is wiped on drop and never
/// moves while it holds the token.
fn bearer(token: &Secret) -> Zeroizing<String> {
    let mut auth = Zeroizing::new(String::with_capacity(token.expose().len() + 7));
    let _ = write!(auth, "Bearer {}", token.expose());
    auth
}

/// A container package name as `ghcr.io` accepts it, nested names included.
fn valid_package(name: &str) -> bool {
    (1..=200).contains(&name.len())
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
        })
}

fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && !login.starts_with('-')
        && !login.ends_with('-')
        && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A GitHub `owner/name`, as Git and GitHub accept it.
#[must_use]
pub fn valid_repository(name: &str) -> bool {
    let mut parts = name.split('/');
    let (Some(owner), Some(repository), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    valid_login(owner)
        && (1..=100).contains(&repository.len())
        && repository != "."
        && repository != ".."
        && repository
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
