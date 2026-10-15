use super::super::selector::summary;
use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::{cloud_panel::CloudConfig, cloud_runtime::publishing::Selection};

fn form() -> Production {
    Production {
        title:"Synthetic build".into(),
        profiles:Some(CloudConfig::parse("version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: registry.example/team/worker\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap()),
        selected_profile:"cpu".into(),
        ..Production::default()
    }
}

#[test]
fn actual_start_and_enter_dispatch_wait_for_reviewed_publishing_metadata() {
    let mut form = form();
    assert!(blocked(&form).is_some());
    assert!(summary::plan(&form).blocked.is_some());
    assert!(confirmation(&form).is_err());
    summary::start(&mut form);
    assert!(!form.launch.submitted);
    form.publishing.key = Some(Key {
        root: "fixture".into(),
        repository: "fixture".into(),
        raw_repository: form.repository.clone(),
        profile_name: form.selected_profile.clone(),
        profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
    });
    form.publishing.info = Some(state::Info {
        enabled: true,
        selection: Some(Selection::Recipe {
            image: "registry.example/team/worker".into(),
        }),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    });
    assert!(blocked(&form).is_none());
    assert!(confirmation(&form).is_ok());
    summary::start(&mut form);
    assert!(form.launch.submitted);
}

#[test]
fn direct_creation_cannot_bypass_the_review_gate() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.cloud_prototype.production = form();
    let error = app.create_production_cloud(&egui::Context::default()).unwrap_err();
    assert!(matches!(
        error,
        horizon_core::cloud_runtime::Error::Invalid(
            "Review the image destination and worker pull access before starting"
        )
    ));
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn cached_publishing_intent_never_crosses_a_profile_or_prebuilt_transition() {
    let mut form = form();
    form.publishing.key = Some(Key {
        root: "fixture".into(),
        repository: "fixture".into(),
        raw_repository: form.repository.clone(),
        profile_name: "cpu".into(),
        profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
    });
    form.publishing.info = Some(state::Info {
        enabled: true,
        selection: Some(Selection::Recipe {
            image: "registry.example/team/worker".into(),
        }),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    });
    assert!(blocked(&form).is_none());
    form.launch.publishing = confirmation(&form).unwrap();
    form.launch.submitted = true;
    form.profiles.as_mut().unwrap().profiles.get_mut("cpu").unwrap().image = "registry.example/team/another".into();
    assert!(blocked(&form).is_some());
    assert!(launch_choice(&form).is_err());
    form.profiles.as_mut().unwrap().profiles.get_mut("cpu").unwrap().build = None;
    assert!(blocked(&form).is_none(), "a prebuilt profile needs no publisher");
    assert!(
        launch_choice(&form).is_err(),
        "the queued build intent needs a fresh request"
    );
    form.launch.publishing = None;
    form.launch.submitted = false;
    assert!(
        launch_choice(&form).unwrap().is_none(),
        "fresh prebuilt intent never uses old publisher metadata"
    );
}

#[test]
fn a_queued_start_retains_its_reviewed_publisher_across_metadata_refresh() {
    let mut form = form();
    form.publishing.key = Some(Key {
        root: "fixture".into(),
        repository: "fixture".into(),
        raw_repository: form.repository.clone(),
        profile_name: "cpu".into(),
        profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
    });
    form.publishing.info = Some(state::Info {
        enabled: true,
        selection: Some(Selection::Recipe {
            image: "registry.example/team/worker".into(),
        }),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    });
    summary::start(&mut form);
    assert!(form.launch.submitted);
    assert!(launch_choice(&form).unwrap().is_some());
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Automatic {
        login: None,
        repository: None,
    });
    assert!(launch_choice(&form).is_err());
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Recipe {
        image: "registry.example/team/worker".into(),
    });
    assert!(
        launch_choice(&form).unwrap().is_some(),
        "the same reviewed choice can continue"
    );
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Recipe {
        image: "registry.example/team/another".into(),
    });
    assert!(
        matches!(launch_choice(&form), Err(runtime::Error::Invalid(CHANGED))),
        "new metadata cannot redirect the queued request"
    );
}

#[test]
fn a_watch_requires_fresh_review_when_the_publisher_changes_or_needs_setup() {
    let mut form = form();
    form.publishing.key = Some(Key {
        root: "fixture".into(),
        repository: "fixture".into(),
        raw_repository: form.repository.clone(),
        profile_name: "cpu".into(),
        profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
    });
    form.publishing.info = Some(state::Info {
        enabled: true,
        selection: Some(Selection::Recipe {
            image: "registry.example/team/worker".into(),
        }),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    });
    form.launch.publishing = confirmation(&form).unwrap();
    form.placement.data_centers = vec!["EU-1".into()];
    let watch = runtime::prices::watch::Selection::new(
        form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
        form.placement.clone(),
    )
    .unwrap();
    form.launch.watch = Some(watch.clone());
    form.launch.watch_quote = Some(0.01);
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Recipe {
        image: "registry.example/team/another".into(),
    });
    assert!(!watch_ready(&mut form));
    assert!(form.launch.watch.is_none());
    assert!(form.launch.watch_quote.is_none());
    assert_eq!(form.publishing.notice.as_deref(), Some(CHANGED));
    assert!(form.publishing.error.is_none());
    form.publishing.error = None;
    form.launch.watch = Some(watch.clone());
    form.launch.watch_quote = Some(0.01);
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Automatic {
        login: None,
        repository: None,
    });
    assert!(!watch_ready(&mut form));
    assert!(
        form.launch.watch.is_none(),
        "setup must not stay disabled behind a waiting watch"
    );
    form.launch.watch = Some(watch);
    form.publishing.error = Some("The configured grant expired".into());
    assert!(!watch_ready(&mut form));
    assert_eq!(form.publishing.error.as_deref(), Some("The configured grant expired"));
    assert!(form.publishing.notice.is_some());
}

#[test]
fn actual_metadata_sync_tracks_profile_changes_and_clears_prebuilt_state() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("source");
    std::fs::create_dir(&repository).unwrap();
    let cloud = root.path().join("cloud");
    let ctx = egui::Context::default();
    let mut form = form();
    form.repository = repository.to_string_lossy().into_owned();
    update_for_root(&mut form, Some(&cloud), &ctx);
    assert!(form.publishing.active());
    assert!(form.publishing.busy());
    let settle = |form: &mut Production| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while form.publishing.busy() {
            update_for_root(form, Some(&cloud), &ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut form);
    assert!(blocked(&form).is_none());
    form.profiles.as_mut().unwrap().profiles.get_mut("cpu").unwrap().image = "registry.example/team/another".into();
    assert!(blocked(&form).is_some());
    update_for_root(&mut form, Some(&cloud), &ctx);
    assert!(form.publishing.busy());
    settle(&mut form);
    assert_eq!(
        launch_choice(&form).unwrap(),
        Some(Selection::Recipe {
            image: "registry.example/team/another".into()
        })
    );
    form.profiles.as_mut().unwrap().profiles.get_mut("cpu").unwrap().build = None;
    update_for_root(&mut form, Some(&cloud), &ctx);
    assert!(!form.publishing.active());
    assert!(form.publishing.info.is_none());
    assert!(confirmation(&form).unwrap().is_none());
    assert!(!cloud.join("settings.json").exists());
}

#[test]
fn start_during_reload_queues_the_shown_choice_but_cannot_dispatch_stale_metadata() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("source");
    std::fs::create_dir(&repository).unwrap();
    let cloud = root.path().join("cloud");
    let ctx = egui::Context::default();
    let mut form = form();
    form.repository = repository.to_string_lossy().into_owned();
    let settle = |form: &mut Production| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            update_for_root(form, Some(&cloud), &ctx);
            if !form.publishing.busy() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut form);
    form.publishing.reload(&ctx);
    assert!(waiting(&form));
    assert!(summary::plan(&form).blocked.is_none());
    summary::start(&mut form);
    assert!(
        form.launch.submitted,
        "Enter and Start must retain the click during a periodic read"
    );
    assert!(form.launch.publishing.is_some());
    assert!(
        launch_choice(&form).is_err(),
        "queued intent is not permission to launch stale metadata"
    );
    settle(&mut form);
    assert!(launch_choice(&form).unwrap().is_some());
    form.publishing.info.as_mut().unwrap().selection = Some(Selection::Recipe {
        image: "registry.example/team/changed".into(),
    });
    assert_eq!(queued_refusal(&form), Some(CHANGED));
}

#[test]
fn stopped_watch_feedback_remains_visible_without_an_enabled_build_selector() {
    fn contains(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(shape) => shape.galley.text().contains(text),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, text)),
            _ => false,
        }
    }
    for no_build in [false, true] {
        let mut form = form();
        if no_build {
            form.profiles.as_mut().unwrap().profiles.get_mut("cpu").unwrap().build = None;
        } else {
            form.publishing.info = Some(state::Info {
                enabled: false,
                selection: Some(Selection::Recipe {
                    image: "registry.example/team/worker".into(),
                }),
                selection_error: None,
                choices: Vec::new(),
                shown_override: None,
                pull_saved: false,
            });
        }
        form.publishing.notice = Some("Review the changed destination".into());
        form.publishing.error = (!no_build).then(|| "Synthetic metadata error".into());
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| show(ui, &mut form))
            .discard_textures();
        for text in ["Stock watch stopped", "Review the changed destination"]
            .into_iter()
            .chain((!no_build).then_some("Synthetic metadata error"))
        {
            assert!(
                output.shapes.iter().any(|shape| contains(&shape.shape, text)),
                "missing {text}, no_build={no_build}"
            );
        }
    }
}

#[test]
fn a_disabled_explicit_recipe_keeps_refresh_silent() {
    fn contains(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(shape) => shape.galley.text().contains("This profile needs an explicit image"),
            egui::Shape::Vec(shapes) => shapes.iter().any(contains),
            _ => false,
        }
    }
    let mut form = form();
    form.publishing.info = Some(state::Info {
        enabled: false,
        selection: Some(Selection::Recipe {
            image: "registry.example/team/worker".into(),
        }),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    });
    let temp = tempfile::tempdir().unwrap();
    form.publishing.key = Some(Key {
        root: temp.path().join("cloud"),
        repository: temp.path().into(),
        raw_repository: form.repository.clone(),
        profile_name: form.selected_profile.clone(),
        profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
    });
    form.publishing.reload(&egui::Context::default());
    let output = egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| show(ui, &mut form))
        .discard_textures();
    assert!(
        !output.shapes.iter().any(|shape| contains(&shape.shape)),
        "a reload does not remove the explicit image"
    );
    form.profiles
        .as_mut()
        .unwrap()
        .profiles
        .get_mut("cpu")
        .unwrap()
        .image
        .clear();
    let output = egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| show(ui, &mut form))
        .discard_textures();
    assert!(
        output.shapes.iter().any(|shape| contains(&shape.shape)),
        "a genuinely missing image still needs guidance"
    );
}
