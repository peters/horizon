//! The fields of the Browser tab, drawn in a headless frame: every setting of
//! the engine has one, and a value that the engine refuses shows its rule.

use horizon_core::Config;

use super::super::{at, frame_of, texts_hold};

#[test]
fn the_tab_draws_a_field_for_every_setting_of_the_engine() {
    let mut config = Config::default();
    let (changed, texts) = frame_of(|ui| super::render(ui, &mut config));

    assert!(!changed, "a frame that a person did not edit changes nothing");
    for expected in [
        "Engines",
        "Chromium",
        "Firefox",
        "Geckodriver",
        "Safaridriver",
        "Live view",
        "Picture quality",
        "Skip frames",
        "Recording",
        "Quality",
        "Frames per second",
        "Compression",
        "Longest side",
        "Passkeys",
    ] {
        assert!(
            texts_hold(&texts, expected),
            "the tab draws a field for {expected}: {texts:?}"
        );
    }
}

#[test]
fn the_tab_shows_the_rule_of_a_value_that_the_engine_refuses() {
    let mut config = Config::default();
    config.browser.video.fps = 99;
    config.browser.quality = 0;
    let (changed, texts) = frame_of(|ui| super::render(ui, &mut config));

    assert!(!changed, "a refused value is not a change of the draft");
    assert!(
        texts_hold(&texts, "browser.quality must be between 1 and 100 (got 0)"),
        "the rule of the picture quality is under its field: {texts:?}"
    );

    config.browser.quality = 60;
    config.browser.video.fps = 99;
    let (_, texts) = frame_of(|ui| super::render(ui, &mut config));
    assert!(
        texts_hold(&texts, "browser.video: video fps must be between 1 and 30"),
        "the rule of the recording is under its fields: {texts:?}"
    );
}

#[test]
fn a_capture_file_limit_is_a_whole_number_of_mebibytes() {
    let mut config = Config::default();
    config.browser.video.max_file_bytes = 64 * 1024 * 1024;
    let (_, texts) = frame_of(|ui| super::render(ui, &mut config));
    assert!(
        at(&texts, "64") && at(&texts, " MiB"),
        "the limit reads in mebibytes: {texts:?}"
    );
    assert_eq!(
        config.browser.video.max_file_bytes,
        64 * 1024 * 1024,
        "a frame that a person did not edit keeps the bytes of the store"
    );

    config.browser.video.max_file_bytes = 3 * 1024 * 1024;
    let (_, texts) = frame_of(|ui| super::render(ui, &mut config));
    assert!(at(&texts, "3"), "{texts:?}");
}

#[test]
fn a_passkey_command_keeps_one_argument_per_line() {
    let mut config = Config::default();
    config.browser.webauthn_command = vec![
        "/opt/my authenticator".to_string(),
        "--dir".to_string(),
        "/var/tmp/auth".to_string(),
    ];
    let (changed, texts) = frame_of(|ui| super::render(ui, &mut config));

    assert!(!changed, "the field draws the command of the settings");
    assert!(
        texts
            .iter()
            .any(|text| text.contains("/opt/my authenticator\n--dir\n/var/tmp/auth")),
        "an argument with a space stays one line: {texts:?}"
    );
    assert_eq!(
        config.browser.webauthn_command.len(),
        3,
        "a frame that a person did not edit keeps every argument"
    );
}
