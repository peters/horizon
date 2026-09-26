//! Where a cloud lives, named on its card: the data center its worker landed in, or
//! the placement chosen for it until a worker exists.
use horizon_core::cloud_panel::CloudLaunch;
use horizon_core::cloud_runtime::state::Deployment;

pub(super) fn where_it_lives(ui: &mut egui::Ui, launch: &CloudLaunch, state: Option<&Deployment>) {
    if let Some(text) = describe(launch, state) {
        ui.small(text);
    }
}

fn describe(launch: &CloudLaunch, state: Option<&Deployment>) -> Option<String> {
    let worker = state.and_then(|state| state.worker.as_ref());
    let landed = worker.and_then(|worker| {
        worker
            .data_center_id
            .clone()
            .or_else(|| worker.network_volume.as_ref()?.data_center_id.clone())
    });
    let placement = &launch.placement;
    let region = placement.region.as_deref();
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
        let europe = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
        };
        assert_eq!(describe(&launch(Placement::default()), None), None);
        assert_eq!(
            describe(&launch(europe.clone()), None).as_deref(),
            Some("Region: Europe")
        );
        let gpu =
            deployment(r#"{"id":"w","name":"n","imageName":"i","desiredStatus":"RUNNING","dataCenterId":"EUR-IS-1"}"#);
        assert_eq!(
            describe(&launch(europe), Some(&gpu)).as_deref(),
            Some("Data center: EUR-IS-1 · Europe")
        );
        let cpu = deployment(
            r#"{"id":"w","name":"n","imageName":"i","desiredStatus":"EXITED","networkVolume":{"id":"v","dataCenterId":"US-MO-2"}}"#,
        );
        assert_eq!(
            describe(&launch(Placement::default()), Some(&cpu)).as_deref(),
            Some("Data center: US-MO-2")
        );
        let one = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
        };
        assert_eq!(
            describe(&launch(one), None).as_deref(),
            Some("Data center: EU-RO-1 · Europe")
        );
    }
}
