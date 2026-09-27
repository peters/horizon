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
    let cpu_flavors = cpu_flavors(&resized, &request.settings)?;
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
    let cpu_flavors = cpu_flavors(&state.profile, &request.settings)?;
    if let Some(spec) = &mut state.spec {
        spec.profile.clone_from(&state.profile);
        spec.cpu_flavors = cpu_flavors;
        spec.gpu_types.clone_from(&request.settings.gpu_types);
        spec.data_centers.clone_from(&request.settings.data_centers);
        store.save(state)?;
    }
    Ok(())
}
pub(super) fn cpu_flavors(profile: &horizon_cloud::Profile, settings: &Settings) -> Result<Vec<String>> {
    if profile.gpu {
        return Ok(settings.cpu_flavors.clone());
    }
    Ok(horizon_cloud::runpod::flavors::for_profile(
        profile,
        &settings.cpu_flavors,
    )?)
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
}
