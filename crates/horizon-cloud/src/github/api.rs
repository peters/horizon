//! REST calls made with a user access token: who the token acts as, and which
//! repositories the app may reach for that user.
use super::{Client, Error, Result, Secret, read};
use serde::Deserialize;
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// The most installation pages read; 100 repositories each.
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
        let installations: Installations = self.get(token, "/user/installations?per_page=100")?;
        let mut names = Vec::new();
        for installation in installations.installations {
            for page in 1..=MAX_PAGES {
                let path = format!(
                    "/user/installations/{}/repositories?per_page=100&page={page}",
                    installation.id
                );
                let answer: Repositories = self.get(token, &path)?;
                let last = answer.repositories.len() < 100;
                for repository in answer.repositories {
                    if !valid_repository(&repository.full_name) {
                        return Err(Error::InvalidResponse);
                    }
                    names.push(repository.full_name.to_ascii_lowercase());
                }
                if last || names.len() as u64 >= answer.total_count {
                    break;
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn get<T: serde::de::DeserializeOwned>(&self, token: &Secret, path: &str) -> Result<T> {
        let mut auth = Zeroizing::new(String::with_capacity(token.expose().len() + 7));
        let _ = write!(auth, "Bearer {}", token.expose());
        let response = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", auth.as_str())
            .call();
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
