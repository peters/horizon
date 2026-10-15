//! The destination in a deployment plan, retained through its confirmation.
use horizon_cloud::image_publishing::Publisher;
use horizon_core::{
    cloud_panel::Profile,
    cloud_runtime::{
        Cancellation, Error, Event, Result,
        command::Runner,
        deployment,
        github::publish::publication as publishing_account,
        publishing::{self, Selection},
        settings::Settings,
        state::Store,
    },
};
use std::path::Path;

pub(super) fn selection(
    repository: &Path,
    name: &str,
    profile: &Profile,
    settings: &Settings,
    state_root: &Path,
    cloud_id: &str,
    cancel: &Cancellation,
) -> Result<Selection> {
    cancel.check()?;
    let repository = repository.canonicalize()?;
    if let Some(state) = Store::snapshot(state_root)? {
        if state.cloud_id != cloud_id || state.repository != repository {
            return Err(Error::Invalid("This state folder belongs to another cloud or checkout"));
        }
        if let Some(record) = state.publication {
            if record.profile != name {
                return Err(Error::Invalid("This cloud retains another named image profile"));
            }
            let selection = retained(&record)?;
            if state.spec.is_none() {
                selection.check_binding(settings)?;
            }
            return Ok(selection);
        }
        if !state.profile.automatic_image() {
            return Ok(Selection::Recipe {
                image: state.profile.image,
            });
        }
    }
    publishing::selection(settings, &repository, name, profile, cancel)
}

pub(super) fn retained(record: &publishing::Publication) -> Result<Selection> {
    match (&record.destination.publisher, &record.account) {
        (Publisher::GitHub { .. }, Some(account)) => Ok(Selection::Personal {
            destination: record.destination.clone(),
            account: account.clone(),
        }),
        (Publisher::Registry { .. }, None) => Ok(Selection::Registry {
            destination: record.destination.clone(),
        }),
        _ => Err(Error::Invalid("Invalid recorded publishing account")),
    }
}

pub(super) fn show(selection: &Selection) {
    match selection.repository() {
        Some(repository) => println!("Worker image destination: {repository}"),
        None => println!("Worker image destination: Automatic. Sign in before the build to choose your account."),
    }
    match selection {
        Selection::Personal { account, .. } => println!("Publishing account: {}", account.login),
        Selection::Automatic { login: Some(login), .. } => {
            println!("Publishing account: {login} (authentication required)");
        }
        Selection::Registry { .. } => println!("Publishing account: the selected machine-local registry grant"),
        Selection::Recipe { .. } | Selection::Automatic { login: None, .. } => {}
    }
}

/// Keeps the image-only operation locked through the build and immutable pull check.
pub(super) struct ImagePreparation {
    _store: Store,
    pub profile: Profile,
    pub working_copy: Option<horizon_core::cloud_runtime::repository::uncommitted::Confirmed>,
    pub publication: Option<publishing_account::Prepared>,
}

pub(super) fn prepare_image(
    request: &deployment::Request,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<ImagePreparation> {
    deployment::prepare_image_record(request)?;
    let store = Store::lock(&request.state_root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("Missing prepared cloud record"))?;
    if state.spec.is_some() {
        return Err(Error::Invalid(
            "This cloud already has a prepared worker image; use its rebuild flow to prepare a replacement",
        ));
    }
    let runner = Runner {
        cancel,
        emit,
        secrets: Vec::new(),
    };
    let publication = publishing::prepare(request, &store, &mut state, &runner)?;
    Ok(ImagePreparation {
        _store: store,
        profile: state.profile,
        working_copy: state.working_copy,
        publication,
    })
}

#[cfg(test)]
#[path = "publishing/tests.rs"]
mod tests;
