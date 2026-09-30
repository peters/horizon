//! Resolve an explicit catalog worker against current prices and machine policy.
use horizon_cloud::{Profile, offers};
use horizon_core::{
    cloud_panel::WorkerChoice,
    cloud_runtime::{self, Cancellation, Error, Result, prices, provider, settings::Settings},
};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) fn arguments(args: &[String]) -> Result<(Vec<String>, Option<PathBuf>)> {
    let mut remaining = Vec::new();
    let mut choice = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--worker-choice" {
            if choice.is_some() {
                return Err(Error::Invalid("Supply one worker choice"));
            }
            choice = Some(PathBuf::from(
                args.next().ok_or(Error::Invalid("Worker choice needs a JSON file"))?,
            ));
        } else {
            remaining.push(arg.clone());
        }
    }
    Ok((remaining, choice))
}

pub(super) fn apply(profile: &Profile, settings: &mut Settings, path: &Path, cancel: &Cancellation) -> Result<Profile> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(32 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 {
        return Err(Error::Invalid("Worker choice is too large"));
    }
    let identity: WorkerChoice =
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("Invalid worker choice JSON"))?;
    let provider = provider::ALL
        .into_iter()
        .find(|provider| provider.id == identity.provider || provider.label == identity.provider)
        .ok_or(Error::Invalid("Unknown worker provider"))?;
    let runpod = if provider.kind == provider::Kind::RunPod {
        Some(prices::price_list(settings, cancel)?)
    } else {
        None
    };
    let hetzner = if provider.kind == provider::Kind::Hetzner {
        prices::hetzner_catalog(settings, cancel)?
    } else {
        None
    };
    let catalog = offers::workers(profile, 1.0, runpod.as_ref(), hetzner.as_ref());
    let offer = catalog
        .offers
        .iter()
        .take(catalog.matching)
        .find(|offer| {
            offer.provider == provider.label && offer.id == identity.id && offer.location == identity.location
        })
        .ok_or(Error::Invalid(
            "Worker choice is not currently offered under this machine's policy and repository requirements",
        ))?;
    let chosen = WorkerChoice::from(offer).for_profile(profile)?;
    chosen
        .placement
        .apply(&mut settings.data_centers, &mut settings.gpu_types);
    settings.placement = Some(chosen.placement);
    let (cpu, memory_gb) = chosen.size.unwrap_or((profile.cpu, profile.memory_gb));
    let candidate = Profile {
        provider: chosen.provider.id.into(),
        cpu,
        memory_gb,
        ..profile.clone()
    };
    candidate
        .validate(false)
        .map_err(|_| cloud_runtime::Error::Invalid("Selected worker cannot run this profile"))?;
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_choice_requires_one_file_and_preserves_command_arguments() {
        let args = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
        let (rest, file) = arguments(&args(&["deploy", "--worker-choice", "offer.json", "demo", "cpu"])).unwrap();
        assert_eq!(rest, ["deploy", "demo", "cpu"]);
        assert_eq!(file, Some(PathBuf::from("offer.json")));
        assert!(arguments(&args(&["--worker-choice"])).is_err());
        assert!(arguments(&args(&["--worker-choice", "one", "--worker-choice", "two"])).is_err());
    }
}
