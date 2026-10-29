use super::*;
use horizon_core::{
    cloud_panel::{CloudConfig, Placement},
    cloud_runtime::prices::{
        Availability, CpuFlavorPrice, DataCenter, Preferences, PriceList, RUNPOD_STORAGE, SizeAvailability,
    },
};

fn form() -> Production {
    let config = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    ).unwrap();
    Production {
        title: "Watch fixture".into(),
        profiles: Some(config),
        selected_profile: "dev".into(),
        placement: Placement {
            data_centers: vec!["EU-1".into()],
            ..Placement::default()
        },
        ..Production::default()
    }
}

fn answer(form: &mut Production, availability: Availability) {
    let profile = form.profiles.as_ref().unwrap().profiles["dev"].clone();
    form.prices.answered(
        PriceList {
            provider: "RunPod",
            cpu: ["cpu3c", "cpu3g", "cpu5c", "cpu5g"]
                .into_iter()
                .map(|id| CpuFlavorPrice {
                    id: id.into(),
                    name: id.into(),
                    per_vcpu_hour: 0.03,
                })
                .collect(),
            gpus: Vec::new(),
            data_centers: vec![DataCenter {
                id: "EU-1".into(),
                region: "EUROPE".into(),
                workspace_storage: true,
                high_performance_storage: false,
                cpus: Vec::new(),
                gpus: Vec::new(),
            }],
            regions: std::collections::BTreeMap::default(),
            storage: RUNPOD_STORAGE,
        },
        Preferences::default(),
        vec![(
            profile,
            SizeAvailability {
                centers: vec![("EU-1".into(), availability)],
            },
        )],
    );
}

/// A draft that a dismissal parked keeps starting the cloud its watch waited for: the
/// parked frame dispatches the submitted launch, as the open dialog does.
#[test]
fn a_parked_draft_starts_the_cloud_its_watch_waited_for() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let mut form = form();
    answer(&mut form, Availability::None);
    arm(&mut form);
    poll(&mut form);
    assert!(form.launch.watch.is_some());
    app.cloud_prototype.production = form;
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.draft = Some(crate::app::cloud_panel::new_cloud_draft::Draft {
        workspace: None,
        position: [0.0, 0.0],
        restored: false,
    });
    answer(&mut app.cloud_prototype.production, Availability::Low);
    app.poll_parked_cloud_draft(&ctx);
    assert!(
        app.cloud_prototype.production.launch.watch.is_none(),
        "the parked frame polls the watch"
    );
    // This fixture has no cloud accounts, so the dispatch stops there. What matters is that
    // it ran: the parked frame says why on the board instead of opening the account dialog
    // under the person.
    assert!(
        app.cloud_prototype
            .error
            .as_deref()
            .is_some_and(|error| error.contains("needs cloud accounts")),
        "the parked launch dispatch reports the missing accounts: {:?}",
        app.cloud_prototype.error
    );
    assert!(
        !app.cloud_prototype.production.setup.open,
        "a parked draft opens no dialog under the person"
    );
}

#[test]
fn only_an_armed_watch_submits_once_when_fresh_stock_returns() {
    let mut form = form();
    answer(&mut form, Availability::High);
    poll(&mut form);
    assert!(!form.launch.submitted);
    answer(&mut form, Availability::None);
    arm(&mut form);
    poll(&mut form);
    assert!(form.launch.watch.is_some() && !form.launch.submitted);
    answer(&mut form, Availability::Low);
    poll(&mut form);
    assert!(form.launch.watch.is_none() && form.launch.submitted);
    form.launch.submitted = false;
    poll(&mut form);
    assert!(!form.launch.submitted, "a failed start does not silently retry");
}

#[test]
fn stale_stock_and_cancelled_or_changed_selections_never_submit() {
    let mut form = form();
    answer(&mut form, Availability::High);
    arm(&mut form);
    form.prices.refresh();
    poll(&mut form);
    assert!(form.launch.watch.is_some() && !form.launch.submitted);
    answer(&mut form, Availability::High);
    form.size = Some((8, 16));
    poll(&mut form);
    assert!(form.launch.watch.is_none() && !form.launch.submitted);
    form.size = None;
    arm(&mut form);
    form.launch = crate::app::cloud_panel::production::launch::State::default();
    poll(&mut form);
    assert!(!form.launch.submitted);
}

#[test]
fn fresh_stock_does_not_consume_a_watch_armed_while_publication_metadata_reloads() {
    let mut form = form();
    form.profiles.as_mut().unwrap().profiles.get_mut("dev").unwrap().build =
        Some(serde_json::from_value(serde_json::json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let ctx = egui::Context::default();
    form.repository = repo.to_str().unwrap().into();
    let key = super::super::publishing::Key {
        root: root.path().join("cloud"),
        repository: repo.clone(),
        raw_repository: repo.to_str().unwrap().into(),
        profile_name: "dev".into(),
        profile: form.profiles.as_ref().unwrap().profiles["dev"].clone(),
    };
    form.publishing.sync(Some(key), &ctx);
    let settle = |form: &mut Production| {
        for _ in 0..200 {
            form.publishing.poll(&ctx);
            if form.publishing.ready() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("fixture publication metadata did not settle");
    };
    settle(&mut form);
    answer(&mut form, Availability::None);
    form.publishing.reload(&ctx);
    assert!(super::super::selector::summary::plan(&form).watchable);
    form.launch.selector.wait_for_stock = true;
    super::super::selector::summary::start(&mut form);
    assert!(form.launch.watch.is_some());
    answer(&mut form, Availability::Low);
    poll(&mut form);
    assert!(form.launch.watch.is_some());
    assert!(!form.launch.submitted);
    settle(&mut form);
    poll(&mut form);
    assert!(form.launch.watch.is_none());
    assert!(form.launch.submitted);
}
