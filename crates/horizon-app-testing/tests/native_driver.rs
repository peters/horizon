use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use horizon_app_testing::Error;
use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::{App, Form, Platform};
use horizon_app_testing::driver::{Launch, NativeDriver};
use horizon_app_testing::recipe::{Action, State, Target};
use horizon_app_testing::tree::References;
use horizon_browser::{ClassicTransport, WebDriverHttpError};
use serde_json::{Value, json};

const IOS_SOURCE: &str = r#"<?xml version="1.0"?><AppiumAUT><XCUIElementTypeApplication name="app" enabled="true" visible="true"><XCUIElementTypeButton name="menu.open" label="Menu" enabled="true" visible="true" x="0" y="10" width="40" height="20"/><XCUIElementTypeSecureTextField name="password" value="synthetic-password" enabled="true" visible="true"/></XCUIElementTypeApplication></AppiumAUT>"#;
const ANDROID_SOURCE: &str = r#"<hierarchy><node class="android.widget.Button" resource-id="menu.open" content-desc="Menu &amp; navigation" bounds="[0,10][40,30]" enabled="true" displayed="true"/><node class="android.widget.EditText" text="synthetic-password" password="true" enabled="true"/></hierarchy>"#;

#[test]
fn both_native_sources_normalize_bounds_and_redact_secure_values() -> Result<(), Error> {
    for source in [IOS_SOURCE, ANDROID_SOURCE] {
        let mut refs = References::default();
        let snapshot = refs.snapshot(source, "no.youpark.app")?;
        let menu = snapshot
            .nodes
            .iter()
            .find(|n| n.identifier == "menu.open")
            .ok_or(Error::TargetMissing)?;
        assert_eq!(menu.semantic.role, "button");
        let bounds = menu.semantic.bounds.ok_or(Error::SourceInvalid)?;
        assert_eq!(
            (bounds.x, bounds.y, bounds.width, bounds.height),
            (0.0, 10.0, 40.0, 20.0)
        );
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-password"));
        assert!(refs.xpath(&Target::Ref(menu.semantic.reference.clone())).is_ok());
    }
    Ok(())
}

#[test]
fn references_cannot_cross_sessions_or_survive_a_new_or_failed_snapshot() -> Result<(), Error> {
    let mut first = References::default();
    let mut second = References::default();
    let snapshot = first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    let target = Target::Ref(snapshot.nodes[0].semantic.reference.clone());
    second.snapshot(IOS_SOURCE, "no.youpark.app")?;
    assert_eq!(second.xpath(&target).err(), Some(Error::ReferenceExpired));
    first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    assert_eq!(first.xpath(&target).err(), Some(Error::ReferenceExpired));
    let fresh = first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    let current = Target::Ref(fresh.nodes[0].semantic.reference.clone());
    assert_eq!(
        first.snapshot("<unclosed>", "no.youpark.app").err(),
        Some(Error::SourceInvalid)
    );
    assert_eq!(first.xpath(&current).err(), Some(Error::ReferenceExpired));
    Ok(())
}

#[test]
fn malformed_sources_and_entity_declarations_never_resolve_targets() {
    for source in [
        "",
        "<a></b>",
        "<a/><b/>",
        "<!DOCTYPE a [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><a name='&x;'/>",
        "<a duplicate='1' duplicate='2'/>",
    ] {
        assert_eq!(
            References::default().snapshot(source, "app").err(),
            Some(Error::SourceInvalid)
        );
    }
}

#[test]
fn duplicate_identifiers_are_ambiguous_instead_of_selecting_a_random_node() -> Result<(), Error> {
    let mut refs = References::default();
    refs.snapshot("<a><b name='duplicate'/><b name='duplicate'/></a>", "app")?;
    assert_eq!(
        refs.xpath(&Target::Identifier("duplicate".into())).err(),
        Some(Error::TargetAmbiguous)
    );
    Ok(())
}

type Call = (String, String, Option<Value>, Duration);

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
    fail_click: bool,
    late_reply: bool,
    screenshot: Option<Vec<u8>>,
    changing_source: bool,
    swapped_elements: bool,
    duplicate_elements: bool,
}

impl ClassicTransport for Fake {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, WebDriverHttpError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.into(), path.into(), body.cloned(), timeout));
        if path == "/session" {
            return Ok(json!({"value":{"sessionId":"test-session"}}));
        }
        if path.ends_with("/source") {
            let reads = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, p, _, _)| p.ends_with("/source"))
                .count();
            return Ok(
                json!({"value":if self.changing_source && reads > 1 { IOS_SOURCE.replace("Menu", "Other") } else { IOS_SOURCE.to_owned() }}),
            );
        }
        if path.ends_with("/elements") {
            if body.is_some_and(|v| v["value"] == "//*[not(self::AppiumAUT or self::hierarchy)]") {
                return Ok(json!({"value": [
                    {"element-6066-11e4-a52e-4f735466cecf":"app-element"},
                    {"element-6066-11e4-a52e-4f735466cecf":if self.swapped_elements { "secure-element" } else if self.duplicate_elements {"app-element"} else {"menu-element"}},
                    {"element-6066-11e4-a52e-4f735466cecf":if self.swapped_elements {"menu-element"} else {"secure-element"}}
                ]}));
            }
            return Ok(
                json!({"value": if body.is_some_and(|v|v["value"]=="missing") {json!([])} else {json!([{"element-6066-11e4-a52e-4f735466cecf":"element-1"}])}}),
            );
        }
        if path.contains("/attribute/") {
            let key = path.rsplit('/').next().unwrap();
            let value = match (path.contains("menu-element"), path.contains("secure-element"), key) {
                (true, _, "type") => "XCUIElementTypeButton",
                (true, _, "name") => "menu.open",
                (true, _, "label") => "Menu",
                (_, true, "type") => "XCUIElementTypeSecureTextField",
                (_, true, "name") => "password",
                (_, _, "type") => "XCUIElementTypeApplication",
                (_, _, "name") => "app",
                _ => "",
            };
            return Ok(json!({"value":value}));
        }
        if path.ends_with("/rect") {
            return Ok(json!({"value":{"x":0,"y":10,"width":40,"height":20}}));
        }
        if path.ends_with("/screenshot") {
            use base64::Engine as _;
            return Ok(
                json!({"value": base64::engine::general_purpose::STANDARD.encode(self.screenshot.as_deref().unwrap_or(&[]))}),
            );
        }
        if path.ends_with("/enabled") || path.ends_with("/displayed") {
            if self.late_reply {
                std::thread::sleep(timeout + Duration::from_millis(2));
            }
            return Ok(json!({"value":true}));
        }
        if path.ends_with("/click") && self.fail_click {
            return Err(WebDriverHttpError::InvalidResponse("private-provider-token".into()));
        }
        Ok(json!({"value":null}))
    }
}

fn launch(platform: Platform) -> Result<Launch, Error> {
    Launch::new(
        Device {
            platform,
            form: Form::Phone,
            model: "Example phone".into(),
            os_version: "27.0".into(),
        },
        App {
            build: vec!["build".into()],
            artifact: "build/app.ipa".into(),
            bundle_id: (platform == Platform::Ios).then(|| "no.youpark.app".into()),
            package: (platform == Platform::Android).then(|| "no.youpark.app".into()),
        },
        BTreeMap::from([("YOUPARK_BASE_URL".into(), "http://localhost:8080".into())]),
        "bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        "private-tunnel".into(),
        "run-1".into(),
    )
}

#[test]
fn native_allocation_uses_app_capabilities_and_platform_specific_launch_values() -> Result<(), Error> {
    for platform in [Platform::Ios, Platform::Android] {
        let fake = Arc::new(Fake::default());
        let mut driver = NativeDriver::allocate(fake.clone(), &launch(platform)?)?;
        driver.close()?;
        driver.close()?;
        let calls = fake.calls.lock().unwrap();
        let caps = calls[0]
            .2
            .as_ref()
            .unwrap()
            .pointer("/capabilities/alwaysMatch")
            .unwrap();
        assert!(caps.get("browserName").is_none());
        assert_eq!(caps["bstack:options"]["local"], true);
        if platform == Platform::Ios {
            assert_eq!(
                caps["appium:processArguments"]["env"]["YOUPARK_BASE_URL"],
                "http://localhost:8080"
            );
        } else {
            assert_eq!(caps["appium:settings"]["disableIdLocatorAutocompletion"], true);
            assert!(
                caps["appium:optionalIntentArguments"]
                    .as_str()
                    .unwrap()
                    .contains("--es 'YOUPARK_BASE_URL' 'http://localhost:8080'")
            );
        }
        assert_eq!(
            calls
                .iter()
                .filter(|(m, p, _, _)| m == "DELETE" && p == "/session/test-session")
                .count(),
            1
        );
    }
    Ok(())
}

#[test]
fn mutation_invalidates_refs_and_provider_errors_are_redacted() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        fail_click: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[2].semantic.reference.clone());
    let error = driver.act(&Action::Tap { target: target.clone() }).unwrap_err();
    assert_eq!(error, Error::TransportFailed);
    assert!(!error.to_string().contains("private-provider-token"));
    assert_eq!(driver.act(&Action::Tap { target }).err(), Some(Error::ReferenceExpired));
    driver.close()?;
    assert_eq!(driver.snapshot().err(), Some(Error::SessionClosed));
    Ok(())
}

#[test]
fn waits_share_one_deadline_and_absence_cannot_satisfy_disabled() -> Result<(), Error> {
    let fake = Arc::new(Fake::default());
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let missing = Target::Identifier("missing".into());
    driver.wait(&missing, State::Hidden, Duration::from_millis(20))?;
    assert_eq!(
        driver.wait(&missing, State::Disabled, Duration::from_millis(20)).err(),
        Some(Error::WaitTimeout)
    );
    let calls = fake.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .filter(|(_, p, _, _)| p.ends_with("/elements"))
            .all(|(_, _, _, t)| *t <= Duration::from_millis(20))
    );
    Ok(())
}

#[test]
fn secure_fields_redact_every_content_channel() -> Result<(), Error> {
    for source in [
        "<XCUIElementTypeSecureTextField name='synthetic-secret' label='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>",
        "<node class='android.widget.EditText' password='true' resource-id='synthetic-secret' content-desc='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>",
    ] {
        let snapshot = References::default().snapshot(source, "app")?;
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-secret"));
        assert!(snapshot.nodes[0].identifier.is_empty());
    }
    Ok(())
}

#[test]
fn refs_use_observed_element_identity_without_requerying_positions() -> Result<(), Error> {
    let fake = Arc::new(Fake::default());
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[2].semantic.reference.clone());
    fake.calls.lock().unwrap().clear();
    driver.act(&Action::Tap { target })?;
    let calls = fake.calls.lock().unwrap();
    assert!(calls.iter().all(|(_, path, _, _)| !path.ends_with("/elements")));
    assert_eq!(
        calls.last().unwrap().1,
        "/session/test-session/element/menu-element/click"
    );
    Ok(())
}

#[test]
fn changing_observation_cannot_publish_references() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        changing_source: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    assert_eq!(driver.snapshot().err(), Some(Error::ReferenceExpired));
    Ok(())
}

#[test]
fn late_success_cannot_pass_a_wait() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        late_reply: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    assert_eq!(
        driver
            .wait(
                &Target::Identifier("menu.open".into()),
                State::Visible,
                Duration::from_millis(10)
            )
            .err(),
        Some(Error::WaitTimeout)
    );
    Ok(())
}

fn image(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut writer = png::Encoder::new(&mut bytes, width, height).write_header().unwrap();
        writer.write_image_data(&vec![0; (width * height) as usize]).unwrap();
    }
    bytes
}

#[test]
fn screenshots_require_complete_bounded_png_images() -> Result<(), Error> {
    let valid = image(2, 2);
    let mut oversized = valid.clone();
    oversized[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    for bytes in [
        vec![0; 24],
        b"\x89PNG\r\n\x1a\nnot-an-image-with-magic".to_vec(),
        valid[..valid.len() - 12].to_vec(),
        oversized,
    ] {
        let fake = Arc::new(Fake {
            screenshot: Some(bytes),
            ..Fake::default()
        });
        let driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
        assert_eq!(driver.screenshot().err(), Some(Error::DriverInvalid));
    }
    let fake = Arc::new(Fake {
        screenshot: Some(valid.clone()),
        ..Fake::default()
    });
    let driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    assert_eq!(driver.screenshot()?, valid);
    Ok(())
}

#[test]
fn reordered_native_ids_between_identical_sources_never_mutate_the_wrong_element() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        swapped_elements: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[2].semantic.reference.clone());
    assert_eq!(driver.act(&Action::Tap { target }).err(), Some(Error::ReferenceExpired));
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, path, _, _)| !path.ends_with("/click"))
    );
    Ok(())
}

#[test]
fn duplicate_native_ids_cannot_bind_distinct_observations() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        duplicate_elements: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    assert_eq!(driver.snapshot().err(), Some(Error::ReferenceExpired));
    Ok(())
}
