use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use horizon_app_testing::Error;
use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::{App, Evidence, Form, Platform};
use horizon_app_testing::driver::{Launch, NativeDriver};
use horizon_app_testing::recipe::{Action, Direction, State, Target};
use horizon_app_testing::tree::References;
use horizon_browser::{ClassicTransport, WebDriverHttpError};
use serde_json::{Value, json};

const IOS_SOURCE: &str = r#"<?xml version="1.0"?><AppiumAUT><XCUIElementTypeApplication name="app" enabled="true" visible="true"><XCUIElementTypeButton name="menu.open" label="Menu" enabled="true" visible="true" x="0" y="10" width="40" height="20"/><XCUIElementTypeSecureTextField name="password" value="synthetic-password" enabled="true" visible="true"/></XCUIElementTypeApplication></AppiumAUT>"#;
const ANDROID_SOURCE: &str = r#"<hierarchy><node class="android.widget.Button" resource-id="menu.open" content-desc="Menu &amp; navigation" bounds="[0,10][40,30]" enabled="true" displayed="true"/><node class="android.widget.EditText" text="synthetic-password" password="true" clickable="true" enabled="true"/></hierarchy>"#;

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
        assert!(snapshot.nodes.iter().any(|node| node.semantic.role == "textbox"));
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
    screenshot_wrapped: bool,
    android_source: bool,
    wrong_app_package: bool,
    invalid_app_activity: bool,
    changing_source: bool,
    swapped_elements: bool,
    duplicate_elements: bool,
    replaced_binding: bool,
    bindings: std::sync::atomic::AtomicUsize,
    android_pair: bool,
    ios_secure_pair: bool,
    quit_reply: Mutex<Option<Value>>,
    disappearing: Option<&'static str>,
    state_reads: std::sync::atomic::AtomicUsize,
    disappear_identity: bool,
    identity_reads: std::sync::atomic::AtomicUsize,
}

impl Fake {
    fn attribute(&self, path: &str) -> Result<Value, WebDriverHttpError> {
        if self.disappear_identity && self.identity_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Err(WebDriverHttpError::WebDriver {
                error: "no such element".into(),
                message: "synthetic disappearing identity".into(),
            });
        }
        let key = path.rsplit('/').next().unwrap();
        if self.android_source {
            return Ok(json!({"value":match key {
                "class"=>"android.widget.Button", "resource-id"=>"menu.open", "content-desc"=>"Menu & navigation", "password"=>"false", _=>"",
            }}));
        }
        if self.ios_secure_pair {
            return Ok(json!({"value":match key {"type"=>"XCUIElementTypeTextField", "name"|"label"=>"same", _=>""}}));
        }
        if self.android_pair {
            return Ok(json!({"value":match key {
                "class" => json!("android.widget.EditText"), "resource-id" | "content-desc" => json!("same"),
                "password" => json!(path.contains("secure-element")), "text" => json!(""), _ => json!(null),
            }}));
        }
        let value = match (
            path.contains("menu-element") || path.contains("replacement-element"),
            path.contains("secure-element"),
            key,
        ) {
            (true, _, "type") => "XCUIElementTypeButton",
            (true, _, "name") => "menu.open",
            (true, _, "label") => {
                if self.changing_source {
                    "Other"
                } else {
                    "Menu"
                }
            }
            (_, true, "type") => "XCUIElementTypeSecureTextField",
            (_, true, "name") => "password",
            (_, _, "type") => "XCUIElementTypeApplication",
            (_, _, "name") => "app",
            _ => "",
        };
        Ok(json!({"value":value}))
    }
    fn source(&self, path: &str) -> Option<Value> {
        if !path.ends_with("/source") {
            return None;
        }
        if self.android_source {
            return Some(json!({"value":ANDROID_SOURCE}));
        }
        if self.disappear_identity && self.identity_reads.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            return Some(json!({"value":"<AppiumAUT><XCUIElementTypeApplication name='app'/></AppiumAUT>"}));
        }
        let reads = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p, _, _)| p.ends_with("/source"))
            .count();
        if self.ios_secure_pair {
            return Some(
                json!({"value":"<AppiumAUT><XCUIElementTypeSecureTextField class='XCUIElementTypeTextField' name='same' label='same'/></AppiumAUT>"}),
            );
        }
        if self.android_pair {
            return Some(
                json!({"value": "<hierarchy><node class='android.widget.EditText' resource-id='same' content-desc='same' password='true'/><node class='android.widget.EditText' resource-id='same' content-desc='same' password='false' text=''/></hierarchy>"}),
            );
        }
        Some(
            json!({"value":if self.changing_source && reads > 1 { IOS_SOURCE.replace("Menu", "Other") } else { IOS_SOURCE.to_owned() }}),
        )
    }
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
        if method == "DELETE" {
            return Ok(self
                .quit_reply
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| json!({"value":null})));
        }
        if let Some(error) = self.disappearing {
            use std::sync::atomic::Ordering;
            if path.ends_with("/displayed") && self.state_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(WebDriverHttpError::WebDriver {
                    error: error.into(),
                    message: "synthetic disappeared element".into(),
                });
            }
            if path.ends_with("/elements") && self.state_reads.load(Ordering::SeqCst) > 0 {
                return Ok(json!({"value":[]}));
            }
        }
        if path == "/session" {
            return Ok(json!({"value":{"sessionId":"test-session"}}));
        }
        if method == "GET" && path == "/session/test-session" {
            return Ok(
                json!({"value":{"appPackage":if self.wrong_app_package {"another.app"} else {"no.youpark.app"},"appActivity":if self.invalid_app_activity {"../other/private"} else {".MainActivity"}}}),
            );
        }
        if path.ends_with("/window/rect") {
            return Ok(json!({"value":{"width":400,"height":800}}));
        }
        if let Some(response) = self.source(path) {
            return Ok(response);
        }
        if path.ends_with("/elements") {
            if self.disappear_identity && self.identity_reads.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                return Ok(json!({"value":[{"element-6066-11e4-a52e-4f735466cecf":"app-element"}]}));
            }
            if body.is_some_and(|v| v["using"] == "xpath") {
                let query = body.unwrap()["value"].as_str().unwrap();
                let id = if self.ios_secure_pair {
                    "plain-element"
                } else if self.android_pair {
                    if query.ends_with("/*[1]") == self.swapped_elements {
                        "plain-element"
                    } else {
                        "secure-element"
                    }
                } else if query == "/*[1]/*[1]/*[1]" || (self.android_source && query == "/*[1]/*[1]") {
                    if self.swapped_elements {
                        "secure-element"
                    } else if self.replaced_binding
                        && self.bindings.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
                    {
                        "replacement-element"
                    } else {
                        "menu-element"
                    }
                } else if query == "/*[1]/*[1]/*[2]" {
                    "secure-element"
                } else {
                    "app-element"
                };
                let elements = if self.duplicate_elements {
                    json!([{"element-6066-11e4-a52e-4f735466cecf":id},{"element-6066-11e4-a52e-4f735466cecf":id}])
                } else {
                    json!([{"element-6066-11e4-a52e-4f735466cecf":id}])
                };
                return Ok(json!({"value":elements}));
            }
            return Ok(
                json!({"value": if body.is_some_and(|v|v["value"]=="missing") {json!([])} else {json!([{"element-6066-11e4-a52e-4f735466cecf":"element-1"}])}}),
            );
        }
        if path.contains("/attribute/") {
            return self.attribute(path);
        }
        if path.ends_with("/rect") {
            return Ok(json!({"value":{"x":0,"y":10,"width":40,"height":20}}));
        }
        if path.ends_with("/screenshot") {
            use base64::Engine as _;
            return Ok({
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(self.screenshot.as_deref().unwrap_or(&[]));
                let encoded = if self.screenshot_wrapped {
                    encoded
                        .as_bytes()
                        .chunks(76)
                        .map(|part| std::str::from_utf8(part).unwrap())
                        .collect::<Vec<_>>()
                        .join("\r\n")
                } else {
                    encoded
                };
                json!({"value":encoded})
            });
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
    launch_policy(platform, Evidence::default())
}

fn launch_policy(platform: Platform, evidence: Evidence) -> Result<Launch, Error> {
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
        evidence,
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
            assert_eq!(caps["bstack:options"]["appiumVersion"], "2.19.0");
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
fn refs_query_only_the_exact_snapshot_target_and_verify_its_identity() -> Result<(), Error> {
    let fake = Arc::new(Fake::default());
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[2].semantic.reference.clone());
    fake.calls.lock().unwrap().clear();
    driver.act(&Action::Tap { target })?;
    let calls = fake.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .filter(|(_, path, _, _)| path.ends_with("/elements"))
            .all(|(_, _, body, _)| body.as_ref().unwrap()["value"] == "/*[1]/*[1]/*[1]")
    );
    assert_eq!(
        calls.last().unwrap().1,
        "/session/test-session/element/menu-element/click"
    );
    Ok(())
}

#[test]
fn changed_semantics_cannot_authorize_a_snapshot_target() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        changing_source: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    assert_eq!(
        driver
            .act(&Action::Tap {
                target: Target::Ref(snapshot.nodes[2].semantic.reference.clone())
            })
            .err(),
        Some(Error::ReferenceExpired)
    );
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
    let wrapped = Arc::new(Fake {
        screenshot: Some(valid.clone()),
        screenshot_wrapped: true,
        ..Fake::default()
    });
    let wrapped_driver = NativeDriver::allocate(wrapped, &launch(Platform::Android)?)?;
    assert_eq!(wrapped_driver.screenshot()?, valid);
    Ok(())
}

#[test]
fn android_identifier_actions_resolve_exact_source_markers_without_native_id_completion() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        android_source: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Android)?)?;
    driver.wait(
        &Target::Identifier("menu.open".into()),
        State::Visible,
        Duration::from_secs(1),
    )?;
    driver.act(&Action::Tap {
        target: Target::Identifier("menu.open".into()),
    })?;
    let calls = fake.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|(_, path, _, _)| path.ends_with("/element/menu-element/click"))
    );
    assert!(
        calls
            .iter()
            .any(|(_, _, body, _)| body.as_ref().is_some_and(|body| body["using"] == "xpath"))
    );
    assert!(
        !calls
            .iter()
            .any(|(_, _, body, _)| body.as_ref().is_some_and(|body| body["using"] == "id"))
    );
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
fn multiple_xpath_results_never_authorize_a_snapshot_target() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        duplicate_elements: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    assert_eq!(
        driver
            .act(&Action::Tap {
                target: Target::Ref(snapshot.nodes[2].semantic.reference.clone())
            })
            .err(),
        Some(Error::TargetAmbiguous)
    );
    Ok(())
}

#[test]
fn recording_opt_out_and_small_scrolls_preserve_declared_behavior() -> Result<(), Error> {
    let fake = Arc::new(Fake::default());
    let mut driver = NativeDriver::allocate(
        fake.clone(),
        &launch_policy(
            Platform::Android,
            Evidence {
                video: false,
                screenshots: true,
                logs_on_failure: false,
            },
        )?,
    )?;
    for distance in [1, 3] {
        driver.act(&Action::Scroll {
            direction: Direction::Up,
            distance,
        })?;
    }
    driver.close()?;
    let calls = fake.calls.lock().unwrap();
    let options = &calls[0].2.as_ref().unwrap()["capabilities"]["alwaysMatch"]["bstack:options"];
    assert_eq!(options["video"], false);
    assert_eq!(options["debug"], true);
    assert_eq!(options["deviceLogs"], false);
    assert_eq!(options["appiumLogs"], false);
    let gestures = calls
        .iter()
        .filter(|(method, path, _, _)| method == "POST" && path.ends_with("/actions"));
    for (call, distance) in gestures.zip([1, 3]) {
        let actions = &call.2.as_ref().unwrap()["actions"][0]["actions"];
        let from = actions[0]["y"].as_i64().unwrap();
        let to = actions[2]["y"].as_i64().unwrap();
        assert_eq!(to - from, distance);
    }
    Ok(())
}

#[test]
fn secure_android_refs_cannot_type_into_a_swapped_plain_field() -> Result<(), Error> {
    for swapped in [false, true] {
        let fake = Arc::new(Fake {
            android_pair: true,
            swapped_elements: swapped,
            ..Fake::default()
        });
        let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Android)?)?;
        let snapshot = driver.snapshot()?;
        let result = driver.act(&Action::Type {
            target: Target::Ref(
                snapshot
                    .nodes
                    .iter()
                    .find(|node| node.semantic.role == "textbox" && node.identifier.is_empty())
                    .unwrap()
                    .semantic
                    .reference
                    .clone(),
            ),
            text: "synthetic-sensitive-input".into(),
        });
        if swapped {
            assert_eq!(result.err(), Some(Error::ReferenceExpired));
            assert!(
                fake.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|(method, path, _, _)| method != "POST" || !path.ends_with("/value"))
            );
        } else {
            result?;
        }
    }
    Ok(())
}

#[test]
fn screenshot_and_device_log_opt_out_are_independent() {
    for (screenshots, logs) in [(false, true), (true, false), (false, false), (true, true)] {
        let fake = Arc::new(Fake::default());
        NativeDriver::allocate(
            fake.clone(),
            &launch_policy(
                Platform::Ios,
                Evidence {
                    video: false,
                    screenshots,
                    logs_on_failure: logs,
                },
            )
            .unwrap(),
        )
        .unwrap();
        let calls = fake.calls.lock().unwrap();
        let options = &calls[0].2.as_ref().unwrap()["capabilities"]["alwaysMatch"]["bstack:options"];
        assert_eq!(options["debug"], screenshots);
        assert_eq!(options["deviceLogs"], logs);
        assert_eq!(options["appiumLogs"], logs);
    }
}
#[test]
fn malformed_quit_acknowledgement_keeps_exact_session_open_for_cleanup_retry() {
    for response in [
        json!({}),
        json!(null),
        json!({"value":{"error":"failed"}}),
        json!({"value":null,"status":13}),
    ] {
        let fake = Arc::new(Fake {
            quit_reply: Mutex::new(Some(response)),
            ..Fake::default()
        });
        let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios).unwrap()).unwrap();
        assert_eq!(driver.close(), Err(Error::DriverInvalid));
        assert!(driver.snapshot().is_ok());
        *fake.quit_reply.lock().unwrap() = None;
        driver.close().unwrap();
        driver.close().unwrap();
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _, _, _)| method == "DELETE")
                .count(),
            2
        );
    }
}
#[test]
fn hidden_identifier_wait_retries_disappearance_after_lookup() {
    for error in ["stale element reference", "no such element"] {
        let fake = Arc::new(Fake {
            disappearing: Some(error),
            ..Fake::default()
        });
        let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios).unwrap()).unwrap();
        driver
            .wait(
                &Target::Identifier("menu.open".into()),
                State::Hidden,
                Duration::from_secs(1),
            )
            .unwrap();
    }
}
#[test]
fn android_editable_values_and_autocomplete_roles_preserve_secure_redaction() {
    let snapshot = References::default().snapshot("<hierarchy><node class='android.widget.EditText' text='populated' clickable='true'/><node class='android.widget.AutoCompleteTextView' text='suggestion' clickable='true'/><node class='android.widget.MultiAutoCompleteTextView' text='hidden-password' password='true' clickable='true'/></hierarchy>", "app").unwrap();
    for node in &snapshot.nodes[1..] {
        assert_eq!(node.semantic.role, "textbox");
    }
    assert_eq!(snapshot.nodes[1].value, "populated");
    assert_eq!(snapshot.nodes[2].value, "suggestion");
    assert_eq!(snapshot.nodes[3].value, "[redacted]");
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("hidden-password"));
}

#[test]
fn snapshot_ref_disappearance_never_satisfies_a_hidden_assertion() {
    let fake = Arc::new(Fake {
        disappear_identity: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake, &launch(Platform::Ios).unwrap()).unwrap();
    let snapshot = driver.snapshot().unwrap();
    let menu = snapshot
        .nodes
        .iter()
        .find(|node| node.identifier == "menu.open")
        .unwrap();
    assert_eq!(
        driver.wait(
            &Target::Ref(menu.semantic.reference.clone()),
            State::Hidden,
            Duration::from_secs(1)
        ),
        Err(Error::ReferenceExpired)
    );
}
#[test]
fn label_wait_reobserves_an_element_that_disappears_during_identity_check() {
    let fake = Arc::new(Fake {
        disappear_identity: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios).unwrap()).unwrap();
    driver
        .wait(&Target::Label("Menu".into()), State::Hidden, Duration::from_secs(1))
        .unwrap();
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, path, _, _)| path.ends_with("/source"))
            .count()
            >= 2
    );
}

#[test]
fn overridden_secure_native_tags_never_expose_any_content_channel() {
    let snapshot = References::default().snapshot("<XCUIElementTypeSecureTextField class='plain' name='synthetic-secret' label='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>", "app").unwrap();
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-secret"));
    assert!(snapshot.nodes[0].identifier.is_empty());
    assert_eq!(snapshot.nodes[0].value, "[redacted]");
}

#[test]
fn finite_coordinates_with_overflowing_dimensions_do_not_publish_invalid_bounds() {
    for bounds in ["[-1e308,0][1e308,1]", "[0,-1e308][1,1e308]"] {
        let source = format!("<node class='android.widget.Button' bounds='{bounds}'/>");
        let snapshot = References::default().snapshot(&source, "app").unwrap();
        assert!(snapshot.nodes[0].semantic.bounds.is_none());
        serde_json::to_string(&snapshot).unwrap();
    }
}

#[test]
fn a_bound_snapshot_target_cannot_switch_to_an_identical_replacement_id() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        replaced_binding: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[2].semantic.reference.clone());
    driver.wait(&target, State::Visible, Duration::from_secs(1))?;
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
fn contradictory_secure_native_tag_cannot_authorize_typing_into_a_plain_field() -> Result<(), Error> {
    let fake = Arc::new(Fake {
        ios_secure_pair: true,
        ..Fake::default()
    });
    let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Ios)?)?;
    let snapshot = driver.snapshot()?;
    let target = Target::Ref(snapshot.nodes[1].semantic.reference.clone());
    assert_eq!(
        driver
            .act(&Action::Type {
                target,
                text: "synthetic-private-input".into()
            })
            .err(),
        Some(Error::ReferenceExpired)
    );
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, path, _, _)| !path.ends_with("/value"))
    );
    Ok(())
}

#[test]
fn android_relaunch_uses_the_observed_owned_activity_and_preserves_launch_extras() -> Result<(), Error> {
    for (wrong_app_package, invalid_app_activity) in [(false, false), (true, false), (false, true)] {
        let fake = Arc::new(Fake {
            wrong_app_package,
            invalid_app_activity,
            ..Fake::default()
        });
        let mut driver = NativeDriver::allocate(fake.clone(), &launch(Platform::Android)?)?;
        let result = driver.act(&Action::Launch {});
        let calls = fake.calls.lock().unwrap();
        let executed = calls.iter().find(|(_, path, _, _)| path.ends_with("/execute/sync"));
        if wrong_app_package || invalid_app_activity {
            assert_eq!(result.err(), Some(Error::DriverInvalid));
            assert!(executed.is_none());
        } else {
            result?;
            let body = executed.unwrap().2.as_ref().unwrap();
            assert_eq!(body["args"][0]["component"], "no.youpark.app/.MainActivity");
            assert_eq!(
                body["args"][0]["extras"][0],
                json!(["s", "YOUPARK_BASE_URL", "http://localhost:8080"])
            );
        }
    }
    Ok(())
}
