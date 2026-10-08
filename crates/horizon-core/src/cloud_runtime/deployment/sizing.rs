//! CPU, memory and machine settings applied until a worker is requested.
use super::{Deployment, Error, Request, Result, Settings, Store};

/// Applies CPU and memory once the deployment fence allows it.
pub(super) fn assign_requested_size(request: &Request, state: &mut Deployment) -> Result<bool> {
    let mut resized = state.profile.clone();
    resized.cpu = request.profile.cpu;
    resized.memory_gb = request.profile.memory_gb;
    if resized != request.profile || (state.profile != resized && !state.resizable()) {
        return Err(Error::Invalid(
            "Cloud is permanently bound to its repository, revision and profile",
        ));
    }
    if state.profile == resized {
        return Ok(false);
    }
    let cpu_flavors = bound_types(&resized, &request.settings, state.spec.as_ref())?;
    if let Some(spec) = &mut state.spec {
        spec.profile.clone_from(&resized);
        spec.cpu_flavors = cpu_flavors;
    }
    state.profile = resized;
    Ok(true)
}

/// Size and machine settings apply until a worker is requested. Flavors are
/// chosen before the image build so an unavailable size fails in seconds.
pub(super) fn refresh_allocation(request: &Request, store: &Store, state: &mut Deployment) -> Result<()> {
    if !state.resizable() {
        return Ok(());
    }
    let cpu_flavors = bound_types(&state.profile, &request.settings, state.spec.as_ref())?;
    let centers = bound_centers(&state.profile, &request.settings, state.spec.as_ref())?;
    if let Some(spec) = &mut state.spec {
        spec.profile.clone_from(&state.profile);
        spec.cpu_flavors = cpu_flavors;
        spec.gpu_types.clone_from(&request.settings.gpu_types);
        spec.data_centers = centers;
        store.save(state)?;
    }
    Ok(())
}
/// The machine types a worker may run on, as its provider tries them.
pub(super) fn cpu_flavors(profile: &horizon_cloud::Profile, settings: &Settings) -> Result<Vec<String>> {
    crate::cloud_runtime::providers::cpu_flavors(profile, settings)
}

/// Where a worker may run, as its provider names places.
pub(super) fn data_centers(profile: &horizon_cloud::Profile, settings: &Settings) -> Result<Vec<String>> {
    crate::cloud_runtime::providers::data_centers(profile, settings)
}

pub(super) fn hetzner(settings: &Settings) -> Result<&crate::cloud_runtime::settings::Hetzner> {
    settings.hetzner.as_ref().ok_or(Error::Invalid(
        "Add a hetzner section to the cloud settings before deploying a Hetzner cloud",
    ))
}

/// Saved Hetzner choices stay narrowed when CLI lifecycle callers load plain settings.
pub(super) fn bound_types(
    profile: &horizon_cloud::Profile,
    settings: &Settings,
    saved: Option<&horizon_cloud::WorkerSpec>,
) -> Result<Vec<String>> {
    if profile.provider == horizon_cloud::provider::HETZNER.id
        && let Some(spec) = saved.filter(|spec| spec.exact_placement)
    {
        if spec.cpu_flavors.len() != 1 {
            return Err(Error::Invalid("An exact Hetzner placement needs one server type"));
        }
        let placement = crate::cloud_panel::Placement {
            cpu_types: spec.cpu_flavors.clone(),
            ..Default::default()
        };
        return hetzner(settings)?.types_for(Some(&placement));
    }
    cpu_flavors(profile, settings)
}

pub(super) fn bound_centers(
    profile: &horizon_cloud::Profile,
    settings: &Settings,
    saved: Option<&horizon_cloud::WorkerSpec>,
) -> Result<Vec<String>> {
    narrow(
        profile,
        data_centers(profile, settings)?,
        saved
            .filter(|spec| spec.exact_placement)
            .map(|spec| spec.data_centers.as_slice()),
    )
}

fn narrow(profile: &horizon_cloud::Profile, configured: Vec<String>, saved: Option<&[String]>) -> Result<Vec<String>> {
    if profile.provider != horizon_cloud::provider::HETZNER.id {
        return Ok(configured);
    }
    let Some(saved) = saved.filter(|saved| !saved.is_empty()) else {
        return Ok(configured);
    };
    let choices: Vec<_> = saved
        .iter()
        .filter(|choice| configured.contains(choice))
        .cloned()
        .collect();
    if choices.is_empty() {
        return Err(Error::Invalid("The saved worker type or location is no longer allowed"));
    }
    Ok(choices)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_profiles_keep_configured_cpu_flavors() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused", "ssh_identity_file":"unused", "docker_config":"unused",
            "registry_pull_auth_id":null, "cpu_flavors":["cpu3c"], "gpu_types":["fixture-gpu"]
        }))
        .unwrap();
        let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
            "provider":"runpod","image":"registry.example.com/worker","cpu":3,"memory_gb":100,"gpu":true
        }))
        .unwrap();
        assert_eq!(cpu_flavors(&profile, &settings).unwrap(), ["cpu3c"]);
    }
    #[test]
    fn plain_settings_refresh_preserves_a_saved_exact_type_and_location() {
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":"/unused", "ssh_identity_file":"/unused", "docker_config":"/unused",
            "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
            "hetzner":{"token_file":"/unused","server_types":["cpx32","cx33"],"locations":["nbg1","hel1"]}
        }))
        .unwrap();
        let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
            "provider":"hetzner","image":"example.invalid/worker","cpu":4,"memory_gb":8
        }))
        .unwrap();
        let mut spec: horizon_cloud::WorkerSpec = serde_json::from_value(serde_json::json!({
            "operation_id":"choice","image_digest":"image","profile":profile,"public_key":"key",
            "registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cx33"],"data_centers":["hel1"],"exact_placement":true
        }))
        .unwrap();
        assert!(settings.placement.is_none());
        assert_eq!(bound_types(&profile, &settings, Some(&spec)).unwrap(), ["cx33"]);
        assert_eq!(bound_centers(&profile, &settings, Some(&spec)).unwrap(), ["hel1"]);
        settings.hetzner.as_mut().unwrap().server_types = vec!["cpx32".into()];
        assert_eq!(bound_types(&profile, &settings, Some(&spec)).unwrap(), ["cx33"]);
        settings.hetzner.as_mut().unwrap().locations = vec!["nbg1".into()];
        assert!(bound_centers(&profile, &settings, Some(&spec)).is_err());
        spec.cpu_flavors = vec!["bad/type".into()];
        assert!(bound_types(&profile, &settings, Some(&spec)).is_err());
        spec.cpu_flavors.clear();
        assert!(bound_types(&profile, &settings, Some(&spec)).is_err());
        spec.cpu_flavors = vec!["cx33".into(), "cpx32".into()];
        assert!(bound_types(&profile, &settings, Some(&spec)).is_err());
        spec.exact_placement = false;
        assert_eq!(bound_types(&profile, &settings, Some(&spec)).unwrap(), ["cpx32"]);
    }
}
