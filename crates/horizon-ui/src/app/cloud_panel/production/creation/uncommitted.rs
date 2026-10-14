//! The uncommitted files a new cloud's image builds from on the checked-out commit: listed
//! under the profile before Start, as a rebuild lists them, and read again when Start is
//! pressed, so a list that changed since is refused.
use super::Production;
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_panel::Build;
use horizon_core::cloud_runtime::{
    self, Cancellation,
    command::Runner,
    repository::{
        self,
        launch::Prepared,
        uncommitted::{Confirmed, Uncommitted, collect},
    },
};
use std::{collections::BTreeMap, path::Path};

/// Shown when a refused file holds back Start; the refusal itself names the file.
const REFUSED: &str = "An uncommitted file in the image's build context is refused; see the note under the profile.";
const CHANGED: &str = "The uncommitted files in the image's build context changed since they were listed. Check the list again, then start the cloud";
const MOVED: &str =
    "The checked-out commit changed since the settings were read. Check them again, then start the cloud";

/// For each profile with a build, its uncommitted files or why they are refused.
pub(in crate::app::cloud_panel::production) type Files = BTreeMap<String, Result<Uncommitted, String>>;

/// The uncommitted files of each build profile in `prepared`, when its image builds from
/// the working copy; nothing for a typed revision that is not checked out.
pub(in crate::app::cloud_panel::production) fn read(prepared: &Prepared, cancel: &Cancellation) -> Files {
    if !prepared.working_copy {
        return Files::new();
    }
    prepared
        .config
        .profiles
        .iter()
        .filter_map(|(name, profile)| {
            let build = profile.build.as_ref()?;
            let files = || -> cloud_runtime::Result<Uncommitted> {
                cancel.check()?;
                collect(&prepared.repository, &prepared.revision, build)
            };
            Some((name.clone(), files().map_err(|error| error.to_string())))
        })
        .collect()
}

/// The selected profile's uncommitted files, or the refusal that names one. Returns
/// whether **Check again** was clicked: fixing a refused file changes neither
/// `.horizon/cloud.yml` nor `HEAD`, so no automatic reread follows.
pub(super) fn section(ui: &mut Ui, form: &Production) -> bool {
    match form.launch.uncommitted.get(&form.selected_profile) {
        None => false,
        Some(Ok(files)) => {
            super::super::cards::rebuild::uncommitted(ui, "new-cloud-uncommitted-files", files.files());
            false
        }
        Some(Err(refusal)) => {
            ui.label(RichText::new(refusal).size(12.5).color(theme::PALETTE_RED()));
            ui.button("Check again").clicked()
        }
    }
}

/// Why Start waits: an uncommitted file of the selected profile's build context is refused.
pub(super) fn blocked(form: &Production) -> Option<&'static str> {
    matches!(form.launch.uncommitted.get(&form.selected_profile), Some(Err(_))).then_some(REFUSED)
}

/// What Start confirms for the selected profile: its recipe and the hash of the listed
/// files; `None` when its image builds from the commit alone.
/// # Errors
/// Refuses a refused file, as [`blocked`] does.
pub(in crate::app::cloud_panel::production) fn confirmed(
    form: &Production,
    build: Option<&Build>,
) -> cloud_runtime::Result<Option<(Build, Confirmed)>> {
    let (Some(build), Some(files)) = (build, form.launch.uncommitted.get(&form.selected_profile)) else {
        return Ok(None);
    };
    let files = files.as_ref().map_err(|_| cloud_runtime::Error::Invalid(REFUSED))?;
    Ok(Some((build.clone(), Confirmed { hash: files.hash() })))
}

/// Refuses a start whose checkout moved from `revision`, or whose uncommitted files for
/// `build` are no longer the ones listed.
/// # Errors
/// As the refusals above, and as [`collect`] refuses.
pub(in crate::app::cloud_panel::production) fn unchanged(
    repository: &Path,
    revision: &str,
    (build, confirmed): &(Build, Confirmed),
    cancel: &Cancellation,
) -> cloud_runtime::Result<()> {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    if repository::resolve_with_runner(repository, "HEAD", &runner)? != revision {
        return Err(cloud_runtime::Error::Invalid(MOVED));
    }
    cancel.check()?;
    if collect(repository, revision, build)?.hash() != confirmed.hash {
        return Err(cloud_runtime::Error::Invalid(CHANGED));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
