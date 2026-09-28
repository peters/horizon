//! Where a cloud lives, named on its card: the data center its worker landed in with
//! its region, or the placement chosen for it until a worker exists, and the same-worker
//! siblings pinned to live beside it.
use super::super::HorizonApp;
use horizon_core::cloud_panel::CloudLaunch;
use horizon_core::cloud_runtime::state::Deployment;

impl HorizonApp {
    /// Fetches the provider's data center list once when a card shows a data center
    /// whose region is not known yet.
    pub(super) fn request_landed_regions(&mut self, ctx: &egui::Context) {
        let production = &mut self.cloud_prototype.production;
        production.prices.poll();
        let missing = production
            .runtimes
            .values()
            .filter_map(|runtime| landed(runtime.state.as_ref()))
            .any(|center| production.prices.region_of(center).is_none());
        if missing && let Some(root) = self.cloud_prototype.root.as_deref() {
            production.prices.request_regions(root, ctx);
        }
    }
}

/// The data center a cloud's worker landed in, if one has.
pub(super) fn landed(state: Option<&Deployment>) -> Option<&str> {
    state?.worker.as_ref()?.data_center()
}

/// Where the cloud lives in a few characters, for its header: the data center its
/// worker landed in, else the one chosen, the chosen region, or how many were chosen.
pub(in crate::app::cloud_panel) fn short(launch: &CloudLaunch, state: Option<&Deployment>) -> Option<String> {
    if let Some(center) = landed(state) {
        return Some(center.to_owned());
    }
    let placement = &launch.placement;
    match placement.data_centers.as_slice() {
        [center] => Some(center.clone()),
        [] => placement.region.clone(),
        many => Some(
            placement
                .region
                .clone()
                .unwrap_or_else(|| format!("{} data centers", many.len())),
        ),
    }
}

/// `region_of` names the region of a data center, when the provider's list is known.
pub(super) fn where_it_lives(
    ui: &mut egui::Ui,
    launch: &CloudLaunch,
    state: Option<&Deployment>,
    region_of: &dyn Fn(&str) -> Option<String>,
) {
    if let Some(text) = describe(launch, state, region_of) {
        ui.small(text);
    }
    if let Some(text) = gpu_choice(launch) {
        ui.small(text);
    }
    for sibling in state
        .and_then(|state| state.siblings.as_ref())
        .into_iter()
        .flat_map(|set| &set.members)
    {
        ui.small(sibling_line(sibling));
    }
}

/// A pinned sibling, read-only: its repository, the commit checked out on the worker and,
/// after a rebuild moved past it, the commit its image layer was built from.
fn sibling_line(sibling: &horizon_core::cloud_runtime::siblings::Sibling) -> String {
    let short = |revision: &str| revision.get(..12).unwrap_or(revision).to_owned();
    let checkout = short(&sibling.revision);
    match &sibling.image_revision {
        Some(image) => format!("Sibling: {} @ {checkout} · image {}", sibling.repository, short(image)),
        None => format!("Sibling: {} @ {checkout}", sibling.repository),
    }
}

/// The GPU types chosen for this cloud in New cloud, when they replace the preferences.
fn gpu_choice(launch: &CloudLaunch) -> Option<String> {
    match launch.placement.gpu_types.as_slice() {
        [] => None,
        [one] => Some(format!("GPU: {one}")),
        many => Some(format!("GPUs: {}", many.join(", "))),
    }
}

fn describe(
    launch: &CloudLaunch,
    state: Option<&Deployment>,
    region_of: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let landed = landed(state).map(str::to_owned);
    let placement = &launch.placement;
    // A cloud placed anywhere learns its region from where its worker landed.
    let region = placement
        .region
        .clone()
        .or_else(|| landed.as_deref().and_then(region_of));
    let region = region.as_deref();
    match (landed, placement.data_centers.as_slice()) {
        (Some(center), _) => Some(region.map_or_else(
            || format!("Data center: {center}"),
            |region| format!("Data center: {center} · {region}"),
        )),
        (None, []) => None,
        (None, [center]) => Some(region.map_or_else(
            || format!("Data center: {center}"),
            |region| format!("Data center: {center} · {region}"),
        )),
        (None, _) => Some(format!("Region: {}", region.unwrap_or("chosen data centers"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_panel::Placement;

    fn launch(placement: Placement) -> CloudLaunch {
        CloudLaunch {
            deployment_started: true,
            id: "placed".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: horizon_core::cloud_panel::CloudConfig::parse(
                "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/team/worker\n    cpu: 4\n    memory_gb: 8\n",
            )
            .unwrap()
            .profiles
            .remove("dev")
            .unwrap(),
            placement,
        }
    }

    #[test]
    fn the_header_names_the_landed_data_center_before_the_choice() {
        let chosen = launch(Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EU-SE-1".into()],
            gpu_types: Vec::new(),
        });
        assert_eq!(short(&chosen, None).as_deref(), Some("Europe"));
        let landed =
            deployment(r#"{"id":"w","name":"w","imageName":"i","desiredStatus":"RUNNING","dataCenterId":"EU-SE-1"}"#);
        assert_eq!(short(&chosen, Some(&landed)).as_deref(), Some("EU-SE-1"));
        let one = launch(Placement {
            data_centers: vec!["US-TX-3".into()],
            ..Placement::default()
        });
        assert_eq!(short(&one, None).as_deref(), Some("US-TX-3"));
        let many = launch(Placement {
            data_centers: vec!["A".into(), "B".into(), "C".into()],
            ..Placement::default()
        });
        assert_eq!(short(&many, None).as_deref(), Some("3 data centers"));
        assert_eq!(short(&launch(Placement::default()), None), None);
    }

    fn deployment(worker: &str) -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version": 1, "cloud_id": "placed", "repository": "/synthetic", "revision": "a".repeat(40),
            "profile": launch(Placement::default()).profile, "stage": "Ready",
            "operation": {"state": "bound", "worker_id": "w"}, "spec": null,
            "worker": serde_json::from_str::<serde_json::Value>(worker).unwrap(), "sessions": [],
        }))
        .unwrap()
    }

    #[test]
    fn the_card_names_the_region_until_a_worker_lands_and_then_its_data_center() {
        let unknown = |_: &str| None;
        let known = |center: &str| (center == "US-MO-2").then(|| "North America".to_owned());
        let europe = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(describe(&launch(Placement::default()), None, &unknown), None);
        assert_eq!(
            describe(&launch(europe.clone()), None, &unknown).as_deref(),
            Some("Region: Europe")
        );
        let gpu =
            deployment(r#"{"id":"w","name":"n","imageName":"i","desiredStatus":"RUNNING","dataCenterId":"EUR-IS-1"}"#);
        assert_eq!(
            describe(&launch(europe), Some(&gpu), &unknown).as_deref(),
            Some("Data center: EUR-IS-1 · Europe")
        );
        let cpu = deployment(
            r#"{"id":"w","name":"n","imageName":"i","desiredStatus":"EXITED","networkVolume":{"id":"v","dataCenterId":"US-MO-2"}}"#,
        );
        assert_eq!(
            describe(&launch(Placement::default()), Some(&cpu), &unknown).as_deref(),
            Some("Data center: US-MO-2")
        );
        // Placed anywhere, the card names the region once the provider's list is known.
        assert_eq!(
            describe(&launch(Placement::default()), Some(&cpu), &known).as_deref(),
            Some("Data center: US-MO-2 · North America")
        );
        assert_eq!(landed(Some(&cpu)), Some("US-MO-2"));
        let one = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(
            describe(&launch(one), None, &unknown).as_deref(),
            Some("Data center: EU-RO-1 · Europe")
        );
    }

    #[test]
    fn the_card_names_each_pinned_sibling_with_its_commit() {
        let sibling = horizon_core::cloud_runtime::siblings::Sibling {
            alias: "consumer".into(),
            repository: "example/consumer".into(),
            directory: "consumer".into(),
            revision: "0123456789abcdef".repeat(2),
            image_revision: None,
            local_repository: "/synthetic/consumer".into(),
            profile: "gpu".into(),
        };
        assert_eq!(sibling_line(&sibling), "Sibling: example/consumer @ 0123456789ab");
        let short = horizon_core::cloud_runtime::siblings::Sibling {
            revision: "abc".into(),
            ..sibling
        };
        assert_eq!(sibling_line(&short), "Sibling: example/consumer @ abc");
        let rebuilt = horizon_core::cloud_runtime::siblings::Sibling {
            image_revision: Some("fedcba9876543210".repeat(2)),
            ..short
        };
        assert_eq!(
            sibling_line(&rebuilt),
            "Sibling: example/consumer @ abc · image fedcba987654"
        );
    }

    #[test]
    fn the_card_names_a_gpu_chosen_for_the_cloud() {
        assert_eq!(gpu_choice(&launch(Placement::default())), None);
        let a5000 = Placement {
            gpu_types: vec!["NVIDIA RTX A5000".into()],
            ..Placement::default()
        };
        assert_eq!(gpu_choice(&launch(a5000)).as_deref(), Some("GPU: NVIDIA RTX A5000"));
    }
}
