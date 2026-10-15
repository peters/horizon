//! A verified account and an operation-private Docker login for one publication.
//! Mutable host sign-ins cannot replace credentials in a prepared operation.
use super::{CLIENT_ID, Client, Error, Kept, LOCK, Result, Runner, STORE, Secret, private_directory, stored};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The provider's immutable account ID and its current namespace name. Neither is a secret.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: u64,
    pub login: String,
}

impl Account {
    /// # Errors
    /// Rejects malformed metadata and another account, even if a login name was reused.
    pub fn check(&self, observed: &Self) -> Result<()> {
        if self.id == 0 || observed.id != self.id {
            return Err(Error::Invalid(
                "This cloud is bound to another image-publishing account. Sign in to the recorded account before retrying",
            ));
        }
        if !stored::valid_login(&self.login) || !stored::valid_login(&observed.login) {
            return Err(Error::Invalid("Invalid image-publishing account metadata"));
        }
        if !self.login.eq_ignore_ascii_case(&observed.login) {
            return Err(Error::Invalid(
                "The recorded image-publishing account was renamed; choose a new destination for a new cloud",
            ));
        }
        Ok(())
    }
}

/// Owns the login until the Docker operation finishes. It cannot be debug-printed or serialized.
pub struct Prepared {
    account: Account,
    config: tempfile::TempDir,
}

impl Prepared {
    #[must_use]
    pub fn account(&self) -> &Account {
        &self.account
    }

    #[must_use]
    pub fn docker_config(&self) -> &Path {
        self.config.path()
    }

    /// Renews the same account's login after a build, before the registry push.
    /// # Errors
    /// Cancellation, another account, unavailable or refused authentication, or a local file error.
    pub fn renew(&self, docker_config: &Path, cloud_id: &str, runner: &Runner<'_>) -> Result<()> {
        self.renew_using(docker_config, cloud_id, runner, &Client::new())
    }

    fn renew_using(&self, docker_config: &Path, cloud_id: &str, runner: &Runner<'_>, client: &Client) -> Result<()> {
        with_credentials_using(
            docker_config,
            cloud_id,
            Some(&self.account),
            runner,
            client,
            |account, token| self.renew_with(&account, &token),
        )
    }

    fn renew_with(&self, account: &Account, token: &Secret) -> Result<()> {
        self.account.check(account)?;
        super::write_auth(self.config.path(), &account.login, token)
    }
}

/// Authorizes publication before a build and pins the Docker login to one operation.
/// An expected account comes from the cloud's record, never repository configuration.
/// # Errors
/// A cancelled or refused sign-in, another account, unreachable provider or local file error.
pub fn prepare(
    docker_config: &Path,
    cloud_id: &str,
    expected: Option<&Account>,
    runner: &Runner<'_>,
) -> Result<Prepared> {
    prepare_using(docker_config, cloud_id, expected, runner, &Client::new())
}

fn prepare_using(
    docker_config: &Path,
    cloud_id: &str,
    expected: Option<&Account>,
    runner: &Runner<'_>,
    client: &Client,
) -> Result<Prepared> {
    if let Some(expected) = expected {
        expected.check(expected)?;
    }
    with_credentials_using(docker_config, cloud_id, expected, runner, client, |account, token| {
        prepare_with(expected, || Ok((account, token)))
    })
}

/// Explicitly chooses a new publishing account through a device sign-in.
/// # Errors
/// Cancellation, refused authorization, an unreachable provider or a local storage error.
pub fn select_account(docker_config: &Path, cloud_id: &str, runner: &Runner<'_>) -> Result<Account> {
    select_account_using(docker_config, cloud_id, runner, &Client::new())
}

fn select_account_using(docker_config: &Path, cloud_id: &str, runner: &Runner<'_>, client: &Client) -> Result<Account> {
    runner.cancel.check()?;
    private_directory(docker_config)?;
    let _lock = stored::lock(&docker_config.join(LOCK), || Ok(runner.cancel.check()?))?;
    runner.cancel.check()?;
    let (user, _) = super::sign_in_as(docker_config, client, cloud_id, None, runner)?;
    Ok(Account {
        id: user.id,
        login: user.login.to_ascii_lowercase(),
    })
}

fn with_credentials_using<T>(
    docker_config: &Path,
    cloud_id: &str,
    expected: Option<&Account>,
    runner: &Runner<'_>,
    client: &Client,
    use_credentials: impl FnOnce(Account, Secret) -> Result<T>,
) -> Result<T> {
    runner.cancel.check()?;
    private_directory(docker_config)?;
    let _lock = stored::lock(&docker_config.join(LOCK), || Ok(runner.cancel.check()?))?;
    runner.cancel.check()?;
    let found =
        stored::current_user(&docker_config.join(STORE), client, CLIENT_ID, None).map_err(|kept| match kept {
            Kept::Unreachable => Error::Invalid("GitHub could not be reached to prepare image publication"),
            Kept::Unconfirmed => Error::Invalid("GitHub did not confirm the image-publishing permission; retry"),
            Kept::Local(error) => error,
        })?;
    runner.cancel.check()?;
    let found = match (expected, found) {
        (Some(expected), Some((user, token))) if user.id == expected.id => {
            expected.check(&Account {
                id: user.id,
                login: user.login.clone(),
            })?;
            Some((user, token))
        }
        (Some(_), Some(_)) => None,
        (_, found) => found,
    };
    let (user, token) = if let Some(found) = found {
        found
    } else {
        if let Some(expected) = expected {
            (runner.emit)(crate::cloud_runtime::Event::Output(format!(
                "This cloud publishes as {}. Sign in to that account; another account cannot replace it.",
                expected.login
            )));
        }
        super::sign_in_as(docker_config, client, cloud_id, expected, runner)?
    };
    runner.cancel.check()?;
    let prepared = use_credentials(
        Account {
            id: user.id,
            login: user.login.to_ascii_lowercase(),
        },
        token,
    )?;
    runner.cancel.check()?;
    Ok(prepared)
}

fn prepare_with(
    expected: Option<&Account>,
    authenticate: impl FnOnce() -> Result<(Account, Secret)>,
) -> Result<Prepared> {
    let (account, token) = authenticate()?;
    account.check(&account)?;
    if let Some(expected) = expected {
        expected.check(&account)?;
    }
    // The existing safe skeleton supplies daemon context and CLI plugins without logins.
    // A nonexistent chain path keeps this construction from reauthenticating another account.
    let empty = super::private_tempdir("horizon-empty-docker-")?;
    let machine = std::env::var_os("DOCKER_CONFIG")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".docker")));
    let (config, _) = super::build_config_with(empty.path(), machine.as_deref())?;
    super::write_auth(config.path(), &account.login, &token)?;
    // The host's shared config remains intact, including credentials of another operation.
    Ok(Prepared { account, config })
}

#[cfg(test)]
mod tests;
