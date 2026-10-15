pub(super) use super::source::automatic;
use super::{Error, Publication, Publisher, Result, grant, grant_id, selected};
use crate::cloud_runtime::{
    command::Runner,
    deployment::Request,
    github::publish,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::Profile;
use std::path::Path;
const NAME_REQUIRED: &str = "Name the selected cloud profile before choosing its image destination";

/// The destination shown before Start. It reads local sign-in metadata only; a build
/// authenticates the account again. Call from a background task, not the render loop.
/// # Errors
/// Invalid local choices, repository metadata or publishing account.
pub fn preview(
    settings: &Settings,
    repository: &Path,
    name: &str,
    profile: &Profile,
    cancel: &horizon_cloud::Cancellation,
) -> Result<Option<String>> {
    super::selection(settings, repository, name, profile, cancel)
        .map(|selection| selection.repository().map(str::to_owned))
}

/// Pins a new cloud's known local choice before admission checks inspect its image.
/// This is intent metadata, not authentication; preparation still verifies the actor before a build.
pub(in crate::cloud_runtime) fn bind_initial(request: &Request, state: &mut Deployment) -> Result<()> {
    if !request.settings.portable_image_publishing
        || state.profile.build.is_none()
        || request.settings.image_publishing.is_empty()
    {
        return Ok(());
    }
    let Some(name) = request.profile_name.as_deref() else {
        return Ok(());
    };
    let repository = state.repository.canonicalize()?;
    let Some(choice) = selected(&request.settings, &repository, name) else {
        return Ok(());
    };
    let selection = choice.selection(&request.settings)?;
    if request
        .publishing_selection
        .as_ref()
        .is_some_and(|shown| *shown != selection)
    {
        return Err(Error::Invalid(
            "The image destination or publishing account changed since it was shown; review it before starting",
        ));
    }
    let (destination, account) = match selection {
        super::Selection::Personal { destination, account } => (destination, Some(account)),
        super::Selection::Registry { destination } => (destination, None),
        _ => return Err(Error::Invalid("Invalid local image publication choice")),
    };
    let record = Publication {
        profile: name.into(),
        requested_image: state.profile.image.clone(),
        destination,
        account,
    };
    state.profile.image.clone_from(&record.destination.repository);
    state.publication = Some(Box::new(record));
    state.version = state.record_version();
    Ok(())
}

/// Resolves a build destination once, records it before preparation, and pins publishing
/// credentials to this operation. Existing records ignore changes to local overrides.
/// # Errors
/// Sign-in refusal, another publishing account, changed bindings, missing read-only
/// worker access or a record that cannot be saved. No build or allocation takes place.
pub fn prepare(
    request: &Request,
    store: &Store,
    state: &mut Deployment,
    runner: &Runner<'_>,
) -> Result<Option<publish::publication::Prepared>> {
    if let (Some(record), Some(shown)) = (&state.publication, &request.publishing_selection) {
        record.check_selection(shown)?;
    }
    if state.spec.is_some() || state.profile.build.is_none() {
        return Ok(None);
    }
    runner.cancel.check()?;
    // Explicit legacy records predate local publication preferences and retain their target.
    if state.publication.is_none() && !state.profile.automatic_image() {
        if request
            .publishing_selection
            .as_ref()
            .is_some_and(|shown| !matches!(shown, super::Selection::Recipe { image } if image == &state.profile.image))
        {
            return Err(Error::Invalid(if request.settings.portable_image_publishing {
                "This legacy cloud retains its recorded image; review its original destination before retrying"
            } else {
                super::STAGED
            }));
        }
        return Ok(None);
    }
    let repository = state.repository.canonicalize()?;
    if state.publication.is_none()
        && !request.settings.portable_image_publishing
        && (state.profile.automatic_image()
            || request
                .profile_name
                .as_deref()
                .is_some_and(|name| selected(&request.settings, &repository, name).is_some()))
    {
        return Err(Error::Invalid(super::STAGED));
    }
    if state.publication.is_none()
        && let Some(expected) = &request.publishing_selection
    {
        let name = request.profile_name.as_deref().ok_or(Error::Invalid(NAME_REQUIRED))?;
        if super::selection(
            &request.settings,
            &state.repository,
            name,
            &state.profile,
            runner.cancel,
        )? != *expected
        {
            return Err(Error::Invalid(
                "The image destination or publishing account changed since it was shown; review it before starting",
            ));
        }
    }
    let prepared = if state.publication.is_some() {
        authorize(state, &request.settings, runner)?
    } else {
        let Some(chosen) = choose(request, state, &repository, runner)? else {
            return Ok(None);
        };
        let publication = chosen.record;
        state.profile.image.clone_from(&publication.destination.repository);
        state.publication = Some(Box::new(publication));
        state.version = state.record_version();
        runner.cancel.check()?;
        store.save(state)?;
        chosen.login
    };
    if let Some(record) = &state.publication {
        (runner.emit)(crate::cloud_runtime::Event::Output(format!(
            "Worker image destination: {}",
            record.destination.repository
        )));
    }
    // Automatic personal packages are private. Never turn them public or lend the
    // publisher's write token to a worker to make a missing pull grant work.
    require_pull(state, &request.settings)?;
    Ok(prepared)
}

struct Chosen {
    record: Publication,
    login: Option<publish::publication::Prepared>,
}

fn choose(request: &Request, state: &Deployment, repository: &Path, runner: &Runner<'_>) -> Result<Option<Chosen>> {
    let name = request.profile_name.as_deref();
    if name.is_some_and(|name| !horizon_cloud::valid_id(name)) {
        return Err(Error::Invalid(NAME_REQUIRED));
    }
    let choice = name.and_then(|name| selected(&request.settings, repository, name));
    let (destination, account, prepared) = if let Some(choice) = choice {
        match choice.selection(&request.settings)? {
            super::Selection::Registry { destination } => (destination, None, None),
            super::Selection::Personal { destination, account } => {
                outside_source(state, &request.settings)?;
                let prepared = publish::publication::prepare(
                    &request.settings.docker_config,
                    &state.cloud_id,
                    Some(&account),
                    runner,
                )?;
                (destination, Some(account), Some(prepared))
            }
            _ => return Err(Error::Invalid("Invalid local image publication choice")),
        }
    } else if state.profile.automatic_image() {
        let name = name.ok_or(Error::Invalid(NAME_REQUIRED))?;
        outside_source(state, &request.settings)?;
        let prepared = publish::publication::prepare(&request.settings.docker_config, &state.cloud_id, None, runner)?;
        if let Some(super::Selection::Automatic { login: Some(shown), .. }) = &request.publishing_selection
            && !shown.eq_ignore_ascii_case(&prepared.account().login)
        {
            return Err(Error::Invalid(
                "The publishing account changed since the plan; review the destination before starting",
            ));
        }
        let destination = automatic(&prepared.account().login, &state.repository, name, runner)?;
        if let Some(super::Selection::Automatic {
            repository: Some(shown),
            ..
        }) = &request.publishing_selection
            && shown != &destination.repository
        {
            return Err(Error::Invalid(
                "The repository image identity changed since it was shown; review it before starting",
            ));
        }
        (destination, Some(prepared.account().clone()), Some(prepared))
    } else {
        return Ok(None);
    };
    let record = Publication {
        profile: name.ok_or(Error::Invalid(NAME_REQUIRED))?.to_owned(),
        requested_image: state.profile.image.clone(),
        destination,
        account,
    };
    Ok(Some(Chosen {
        record,
        login: prepared,
    }))
}

pub(in crate::cloud_runtime) fn authorize(
    state: &Deployment,
    settings: &Settings,
    runner: &Runner<'_>,
) -> Result<Option<publish::publication::Prepared>> {
    let Some(record) = &state.publication else {
        return Ok(None);
    };
    record.validate(&state.profile)?;
    match &record.destination.publisher {
        Publisher::GitHub { .. } => {
            outside_source(state, settings)?;
            publish::publication::prepare(
                &settings.docker_config,
                &state.cloud_id,
                record.account.as_ref(),
                runner,
            )
            .map(Some)
        }
        Publisher::Registry { binding: expected } => {
            let binding = grant(settings, &record.destination.repository)?;
            if grant_id(binding)? != *expected {
                return Err(Error::Invalid(
                    "The publishing credential changed; restore this cloud's original binding before rebuilding",
                ));
            }
            Ok(None)
        }
    }
}

fn outside_source(state: &Deployment, settings: &Settings) -> Result<()> {
    check_auth_storage(settings, &state.repository)
}

/// Refuses authentication storage within the repository before any sign-in or write.
/// # Errors
/// Invalid source paths or authentication storage within the build source.
pub fn check_auth_storage(settings: &Settings, repository: &Path) -> Result<()> {
    crate::cloud_runtime::registry::ensure_outside_source(&settings.docker_config, &repository.canonicalize()?)
}

pub(super) fn require_pull(state: &Deployment, settings: &Settings) -> Result<()> {
    let image = &state.profile.image;
    let bound = settings
        .registries
        .as_ref()
        .map(|config| config.select(image))
        .transpose()?
        .flatten();
    if state
        .publication
        .as_ref()
        .is_some_and(|record| record.account.is_some())
        && bound.is_none()
    {
        return Err(Error::Invalid(
            "The personal worker image is private. Add its separate read-only pull credential in Container registry before building; the destination is recorded on this cloud",
        ));
    }
    crate::cloud_runtime::providers::admit(settings, &state.profile, image, &state.operation)
}
