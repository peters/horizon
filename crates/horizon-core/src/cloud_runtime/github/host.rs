//! This computer's own sign-in for the connected GitHub App. **New cloud** lists the
//! repositories the app reaches with it and clones a private one with it, so no personal
//! access token is needed. It is one more chain of the same app, signed in the way the
//! app's setting says (one Authorize click, or none in Automatic), renewed before each use
//! and kept private beside the app's secret.
use super::{
    Error, Event, Mode, Prompt, Result, Runner, Settings, signin,
    stored::{self, Kept},
};
use crate::cloud_runtime::Cancellation;
use horizon_cloud::github::{Client, Secret};
use std::path::{Path, PathBuf};

const DISCONNECTED: &str = "GitHub was disconnected in Horizon's settings. Connect it again there to use it here.";

/// The start of the name of each sign-in of this computer, where a cloud's sign-in uses its
/// cloud. Each sign-in has its own, so nothing that ends one ends another.
const SIGN_IN: &str = "horizon-this-computer";

/// Where this computer's chain for the app of `settings` is kept. A chain of another app is
/// never used for this one.
fn path(root: &Path, settings: &Settings) -> PathBuf {
    root.join("credentials")
        .join(format!("github-host-{}.json", settings.app_id))
}

/// The name of the file that keeps the repositories last listed with this computer's
/// sign-in for the app `app_id`, beside the sign-in itself.
fn listed_name(app_id: u64) -> String {
    format!("github-host-{app_id}-repositories.json")
}

fn listed(root: &Path, settings: &Settings) -> PathBuf {
    root.join("credentials").join(listed_name(settings.app_id))
}

/// Holds the lock of this computer's chain, so two Horizon windows never renew it at once:
/// a renewal ends the old chain, and the second would lose it.
fn lock(root: &Path, settings: &Settings) -> Result<std::fs::File> {
    let directory = root.join("credentials");
    stored::private_directory(&directory)?;
    stored::lock(&directory.join(format!("github-host-{}.lock", settings.app_id)), || {
        Ok(())
    })
}

/// Whether the machine settings in `root` still name the app of `settings`. Read leniently,
/// for its ID only; settings that cannot be read do not name it.
fn configured(root: &Path, settings: &Settings) -> bool {
    std::fs::read(root.join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.pointer("/github/app_id").and_then(serde_json::Value::as_u64))
        == Some(settings.app_id)
}

/// The account and a usable access token of this computer's sign-in, renewed when close
/// to its expiry, or `None` when it must sign in first. A Disconnect since the settings
/// were read ends it.
/// # Errors
/// GitHub out of reach or not confirming the sign-in, which is then kept, or a local file
/// that cannot be read or written.
pub fn current(root: &Path, settings: &Settings) -> Result<Option<(String, Secret)>> {
    renewed(root, settings, &Client::new())
}

fn renewed(root: &Path, settings: &Settings, client: &Client) -> Result<Option<(String, Secret)>> {
    let _lock = lock(root, settings)?;
    if !configured(root, settings) {
        stored::forget(&listed(root, settings))?;
        return stored::forget(&path(root, settings));
    }
    // A chain from a web sign-in renews with the secret whatever the setting says now; a
    // device chain needs none, and the secret is then not read.
    stored::current(
        &path(root, settings),
        client,
        &settings.client_id,
        Some(&|| settings.client_secret()),
    )
    .map_err(|kept| match kept {
        Kept::Unreachable => Error::Invalid("GitHub could not be reached. Check the network and try again."),
        Kept::Unconfirmed => Error::Invalid("GitHub did not confirm this computer's sign-in. Try again."),
        Kept::Local(error) => error,
    })
}

/// Signs this computer in for the app of `settings`. `show` gets the code to approve, or
/// the page to open in Automatic; only `cancel` ends it, so a sign-in that another request
/// or Horizon window ends never ends this one. Returns the account and its token, or why
/// GitHub ended the sign-in.
/// # Errors
/// A local file that cannot be written, or a cancelled sign-in, which stores nothing.
pub fn sign_in(
    root: &Path,
    settings: &Settings,
    cancel: &Cancellation,
    show: &dyn Fn(Prompt),
) -> Result<std::result::Result<(String, Secret), String>> {
    // A dialog left open after a Disconnect in another window asks GitHub for nothing.
    if !configured(root, settings) {
        return Ok(Err(DISCONNECTED.into()));
    }
    let emit = |event| {
        if let Event::GitHub(prompt) = event {
            show(prompt);
        }
    };
    let runner = Runner {
        cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let client = Client::new();
    let name = format!("{SIGN_IN}-{}", uuid::Uuid::new_v4().simple());
    let chain = match signin::chain(settings, &name, &client, &runner) {
        Ok(chain) => chain,
        Err(signin::Ended::Error(error)) => return Err(error),
        Err(signin::Ended::Skipped) => return Ok(Err("Skipped.".into())),
        Err(signin::Ended::Reason(reason)) => return Ok(Err(reason)),
    };
    let user = match client.user(&chain.access_token) {
        Ok(user) => user,
        Err(error) => return Ok(Err(error.to_string())),
    };
    // Locked only to store it: the sign-in itself waits for the person. A Disconnect while
    // it waited keeps the chain out, as does a request that ended meanwhile.
    cancel.check()?;
    let _lock = lock(root, settings)?;
    cancel.check()?;
    if !configured(root, settings) {
        return Ok(Err(DISCONNECTED.into()));
    }
    stored::save(
        &path(root, settings),
        &user.login,
        &chain,
        settings.mode == Mode::Automatic,
    )?;
    Ok(Ok((user.login, chain.access_token)))
}

/// Every `owner/name` the app is installed on and this computer's account reaches, in
/// lowercase and sorted.
/// # Errors
/// GitHub out of reach or refusing the token.
pub fn repositories(token: &Secret) -> std::result::Result<Vec<String>, String> {
    let mut found = Client::new()
        .installed_repositories(token)
        .map_err(|error| error.to_string())?;
    found.sort();
    found.dedup();
    Ok(found)
}

/// A kept list and the account it was listed for.
#[derive(serde::Serialize, serde::Deserialize)]
struct Listed {
    login: String,
    repositories: Vec<String>,
}

/// The account and the repositories last listed with this computer's sign-in for the app
/// of `settings`, so **New cloud** shows them before GitHub answers again. `None` while
/// this computer is not signed in to that app, when the list belongs to another account,
/// or when no list was kept, it cannot be read or the lock cannot be taken.
#[must_use]
pub fn kept_repositories(root: &Path, settings: &Settings) -> Option<(String, Vec<String>)> {
    // Read with the sign-in under its lock, so a sign-in as another account in another
    // window never falls between the two reads.
    let _lock = lock(root, settings).ok()?;
    if !configured(root, settings) {
        return None;
    }
    let login = stored::load(&path(root, settings))?.login;
    let bytes = stored::read_private(&listed(root, settings)).ok()??;
    let kept: Listed = serde_json::from_slice(&bytes).ok()?;
    (kept.login == login).then_some((kept.login, kept.repositories))
}

/// Keeps `repositories`, listed for the account `login`, for the next **New cloud**: private
/// beside the sign-in and under its lock. A Disconnect forgets them with it, and a list
/// that arrives after one, after a sign-in as another account, or for a request that was
/// `cancel`led and replaced, is not kept: checked under the lock, so a newer list written
/// after the cancel is never overwritten by an older one. A list too large to read back is
/// not kept, and replaces no older one.
///
/// Returns whether `login` is still this computer's sign-in, so a list of an account that
/// another window replaced in the meantime is not shown either.
/// # Errors
/// A file that cannot be written.
pub fn keep_repositories(
    root: &Path,
    settings: &Settings,
    login: &str,
    repositories: &[String],
    cancel: &Cancellation,
) -> Result<bool> {
    let _lock = lock(root, settings)?;
    let current =
        configured(root, settings) && stored::load(&path(root, settings)).is_some_and(|chain| chain.login == login);
    if !current || cancel.is_cancelled() {
        return Ok(current);
    }
    let kept = Listed {
        login: login.to_owned(),
        repositories: repositories.to_vec(),
    };
    let bytes = serde_json::to_vec(&kept).map_err(|_| Error::Json)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > stored::MAX_FILE {
        stored::forget(&listed(root, settings))?;
        return Ok(true);
    }
    stored::write_private(&root.join("credentials"), &listed_name(settings.app_id), &bytes)?;
    Ok(true)
}

/// Whether `login` is still this computer's sign-in for the app of `settings`, read under
/// the sign-in's lock: a list that failed for an account that another window has replaced
/// since is not shown as that account's either. A lock that cannot be taken says no, so
/// an unsure answer never shows one account's names to another.
#[must_use]
pub fn still_signed_in_as(root: &Path, settings: &Settings, login: &str) -> bool {
    let Ok(_lock) = lock(root, settings) else {
        return false;
    };
    configured(root, settings) && stored::load(&path(root, settings)).is_some_and(|chain| chain.login == login)
}

/// Forgets the kept list when this computer turns out not to be signed in. Checked again
/// under the lock: a request that was `cancel`led and replaced, or a sign-in that another
/// window finished since, keeps the list.
/// # Errors
/// A file that cannot be removed.
pub fn forget_repositories(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<()> {
    let _lock = lock(root, settings)?;
    if cancel.is_cancelled() || (configured(root, settings) && stored::load(&path(root, settings)).is_some()) {
        return Ok(());
    }
    stored::forget(&listed(root, settings)).map(|_| ())
}

/// Disconnects the app of `settings`: forgets this computer's sign-in for it and runs
/// `save`, which stores the settings without it, both under the sign-in's lock. No renewal
/// or sign-in in another Horizon window can store a chain between the two, and one that
/// comes after finds the app gone.
/// # Errors
/// A file that cannot be removed, which leaves the settings unsaved, or `save`'s error.
pub fn disconnect<T>(root: &Path, settings: &Settings, save: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = lock(root, settings)?;
    stored::forget(&listed(root, settings))?;
    stored::forget(&path(root, settings))?;
    save()
}

#[cfg(test)]
mod tests;
