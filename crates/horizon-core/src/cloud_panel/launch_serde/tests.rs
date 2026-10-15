use super::*;
use serde_json::{Value, json};

// The previous reader requires these outer fields and ignores additional fields.
#[derive(Deserialize)]
struct PreviousLaunch {
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "profile")]
    _profile: horizon_cloud::Profile,
}

fn legacy() -> Value {
    json!({
        "deployment_started":false,"id":"fixture","revision":"a".repeat(40),"profile_name":"cpu",
        "profile":{"provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4}
    })
}

fn selected() -> CloudLaunch {
    let mut launch: CloudLaunch = serde_json::from_value(legacy()).unwrap();
    launch.publishing_selection = Some(Selection::Personal {
        destination: horizon_cloud::image_publishing::Destination::automatic_github(
            "synthetic-user",
            "git.example/team/project",
            "cpu",
        )
        .unwrap(),
        account: crate::cloud_runtime::github::publish::publication::Account {
            id: 17,
            login: "synthetic-user".into(),
        },
    });
    launch
}

#[test]
fn legacy_launches_keep_their_flat_encoding() {
    let mut value = legacy();
    let launch: CloudLaunch = serde_json::from_value(value.clone()).unwrap();
    value["profile"] = serde_json::to_value(&launch.profile).unwrap();
    assert_eq!(serde_json::to_value(launch).unwrap(), value);
}

#[test]
fn publisher_choices_are_fenced_until_launch_can_consume_them() {
    let launch = selected();
    let value = serde_json::to_value(&launch).unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(value["launch"]["publishing_selection"]["account"]["id"], 17);
    assert!(serde_json::from_value::<CloudLaunch>(value.clone()).is_err());
    assert!(serde_json::from_value::<PreviousLaunch>(legacy()).is_ok());
    assert!(serde_json::from_value::<PreviousLaunch>(value).is_err());
}

#[test]
fn incomplete_and_downgraded_choices_are_refused() {
    let value = serde_json::to_value(selected()).unwrap();
    let mut unsupported = value.clone();
    unsupported["version"] = json!(3);
    assert!(serde_json::from_value::<CloudLaunch>(unsupported).is_err());
    let mut absent = value.clone();
    absent["launch"].as_object_mut().unwrap().remove("publishing_selection");
    assert!(serde_json::from_value::<CloudLaunch>(absent).is_err());
    assert!(serde_json::from_value::<CloudLaunch>(value["launch"].clone()).is_err());
    let mut downgraded = value;
    downgraded["version"] = json!(1);
    assert!(serde_json::from_value::<CloudLaunch>(downgraded).is_err());
}

fn recipe() -> CloudLaunch {
    let mut launch: CloudLaunch = serde_json::from_value(legacy()).unwrap();
    launch.publishing_selection = Some(Selection::Recipe {
        image: launch.profile.image.clone(),
    });
    launch
}

#[test]
fn recipe_confirmations_stay_flat_and_preserve_the_image_for_older_readers() {
    let launch = recipe();
    let value = serde_json::to_value(&launch).unwrap();
    assert!(value.get("version").is_none());
    assert_eq!(value["publishing_selection"]["kind"], "recipe");
    assert!(serde_json::from_value::<PreviousLaunch>(value.clone()).is_ok());
    assert_eq!(serde_json::from_value::<CloudLaunch>(value).unwrap(), launch);
    let yaml = serde_yaml::to_string(&launch).unwrap();
    assert!(serde_yaml::from_str::<PreviousLaunch>(&yaml).is_ok());
}

#[test]
fn yaml_keeps_recipe_confirmations_but_refuses_publisher_envelopes() {
    let launch = recipe();
    let yaml = serde_yaml::to_string(&launch).unwrap();
    assert_eq!(serde_yaml::from_str::<CloudLaunch>(&yaml).unwrap(), launch);
    let yaml = serde_yaml::to_string(&selected()).unwrap();
    assert!(serde_yaml::from_str::<CloudLaunch>(&yaml).is_err());
}

#[test]
fn legacy_unknown_keys_are_compatible_but_cannot_hide_an_envelope_or_retarget_a_recipe() {
    let mut value = legacy();
    value["future_note"] = json!({"nested":true});
    assert!(serde_json::from_value::<CloudLaunch>(value.clone()).is_ok());
    let mut mixed = value;
    mixed["version"] = json!(3);
    mixed["launch"] = serde_json::to_value(selected()).unwrap()["launch"].clone();
    assert!(serde_json::from_value::<CloudLaunch>(mixed.clone()).is_err());
    assert!(serde_yaml::from_str::<CloudLaunch>(&serde_yaml::to_string(&mixed).unwrap()).is_err());
    let mut wrong = serde_json::to_value(recipe()).unwrap();
    wrong["publishing_selection"]["image"] = json!("registry.example/team/different");
    assert!(serde_json::from_value::<CloudLaunch>(wrong).is_err());
}

#[test]
fn reserved_null_keys_cannot_downgrade_a_publisher_envelope() {
    for key in ["version", "launch"] {
        let mut value = legacy();
        value[key] = Value::Null;
        assert!(serde_json::from_value::<CloudLaunch>(value.clone()).is_err());
        assert!(serde_yaml::from_str::<CloudLaunch>(&serde_yaml::to_string(&value).unwrap()).is_err());
    }
}

#[test]
fn malformed_saved_fields_do_not_echo_their_values_in_decode_errors() {
    let mut value = legacy();
    value["profile"]["gpu"] = json!("synthetic-private-marker");
    let error = serde_json::from_value::<CloudLaunch>(value.clone())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-private-marker"));
    let error = serde_yaml::from_str::<CloudLaunch>(&serde_yaml::to_string(&value).unwrap())
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-private-marker"));
}

#[test]
fn inconsistent_recipe_confirmations_are_refused_before_serialization() {
    let mut changed_profile = recipe();
    changed_profile.profile.image = "registry.example/team/different".into();
    let mut changed_confirmation = recipe();
    changed_confirmation.publishing_selection = Some(Selection::Recipe {
        image: "registry.example/team/different".into(),
    });
    for launch in [changed_profile, changed_confirmation] {
        assert!(serde_json::to_value(&launch).is_err());
        assert!(serde_yaml::to_string(&launch).is_err());
        let mut json = Vec::new();
        assert!(serde_json::to_writer(&mut json, &launch).is_err());
        assert!(
            json.is_empty(),
            "the launch serializer emitted a partial recipe before validation"
        );
    }
}
