//! REST calls made with a user access token: who the token acts as, and which
//! repositories the app may reach for that user. A repository's size can also be read
//! without one.
use super::{Client, Error, Result, Secret, read};
use serde::Deserialize;
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// The most pages read of installations, and of each installation's repositories; 100
/// entries each. Beyond it the answer is an error, never partial data.
const MAX_PAGES: u32 = 20;

/// The account a token acts as. `email` is GitHub's private commit address, so a
/// commit never shows the person's own address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
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
        if !valid_login(&fields.login) {
            return Err(Error::InvalidResponse);
        }
        let name = fields
            .name
            .filter(|name| !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control))
            .unwrap_or_else(|| fields.login.clone());
        Ok(User {
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
            let answer: Installations = self.get(token, &format!("/user/installations?per_page=100&page={page}"))?;
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
                let answer: Repositories = self.get(token, &path)?;
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

    fn get<T: serde::de::DeserializeOwned>(&self, token: &Secret, path: &str) -> Result<T> {
        self.request(Some(token), path)
    }

    fn request<T: serde::de::DeserializeOwned>(&self, token: Option<&Secret>, path: &str) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json");
        let mut auth = Zeroizing::new(String::new());
        if let Some(token) = token {
            auth.reserve(token.expose().len() + 7);
            let _ = write!(auth, "Bearer {}", token.expose());
            request = request.header("Authorization", auth.as_str());
        }
        let response = request.call();
        let answer = read(response)?;
        match answer.status {
            200 => answer.parse(),
            401 => Err(Error::Revoked),
            status => Err(Error::Refused(format!("HTTP {status}"))),
        }
    }
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
