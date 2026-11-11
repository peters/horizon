//! The settings of an earlier Horizon reach this build.
//!
//! `tests/fixtures/upgrade/v0.2.7` and `tests/fixtures/upgrade/main` hold the
//! YAML settings files that those versions wrote: the default file of each
//! version, and one file with every section set to a value that is not the
//! default. The test copies a file into a private home, starts the settings of
//! that home, and reads every value of it.

use std::fs;
use std::path::{Path, PathBuf};

use horizon_core::{AppearanceTheme, Config, config_store::ConfigStore};

fn fixture(version: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/upgrade")
        .join(version)
        .join(name)
}

/// A private home that holds `text` as the YAML settings file of a release, and
/// the path of the settings file of this build in that home.
fn home_with(version: &str, name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = root.path().join(".horizon");
    fs::create_dir_all(&home).expect("the Horizon home");
    let yaml = home.join("config.yaml");
    fs::copy(fixture(version, name), &yaml).expect("the settings file of the release");
    let settings = home.join("config.json");
    (root, yaml, settings)
}

fn stored(settings: &Path) -> Config {
    ConfigStore::resolve(settings)
        .load()
        .expect("the store of the home")
        .expect("the import wrote the store")
        .config
}

#[test]
fn the_default_file_of_v027_imports_with_every_value() {
    let (_root, yaml, settings) = home_with("v0.2.7", "config-default.yaml");
    let before = fs::read(&yaml).expect("the file before the start");

    let config = Config::load_from_home(&settings).expect("the settings of v0.2.7");

    assert_eq!(config.version, 12, "the migrations ran to the current version");
    assert_eq!(config.appearance.theme, AppearanceTheme::Auto);
    assert_eq!(config.window.width, 1600.0);
    assert_eq!(config.window.height, 1000.0);
    assert_eq!(config.window.x, None);
    assert_eq!(config.window.y, None);
    assert_eq!(config.shortcuts.command_palette, "Ctrl+Shift+K");
    assert_eq!(config.shortcuts.search, "Ctrl+Shift+F");
    assert_eq!(config.overlays.minimap_height, 180.0);
    assert_eq!(config.overlays.minimap_width, 320.0);
    assert!(!config.features.organize_workspaces_on_session_load);
    assert!(!config.features.sidebar_accordion);
    assert!(config.workspaces.is_empty());

    assert_eq!(
        fs::read(&yaml).expect("the file after the start"),
        before,
        "the YAML file keeps every byte"
    );
    assert!(
        ConfigStore::resolve(&settings).path().is_file(),
        "the import writes the store of the home"
    );
    assert!(!settings.exists(), "the import writes no settings file of this build");

    let config = stored(&settings);
    assert_eq!(config.appearance.theme, AppearanceTheme::Auto);
}

/// The window, the appearance and the shortcuts of both files with non-default
/// sections. Both builds wrote the same values, so one helper reads them.
fn assert_window_look_and_shortcuts(config: &Config, width: f32, height: f32, x: f32, y: f32) {
    assert_eq!(config.version, 12);
    assert_eq!(config.window.width, width);
    assert_eq!(config.window.height, height);
    assert_eq!(config.window.x, Some(x));
    assert_eq!(config.window.y, Some(y));
    assert_eq!(config.appearance.theme, AppearanceTheme::Light);

    assert_eq!(config.shortcuts.command_palette, "Ctrl+Alt+K");
    assert_eq!(config.shortcuts.new_terminal, "Ctrl+Alt+N");
    assert_eq!(config.shortcuts.focus_active_workspace, "Ctrl+Alt+W");
    assert_eq!(config.shortcuts.fit_active_workspace, "Ctrl+Alt+9");
    assert_eq!(config.shortcuts.open_remote_hosts, "Ctrl+Alt+H");
    assert_eq!(config.shortcuts.toggle_sessions, "Ctrl+Alt+J");
    assert_eq!(config.shortcuts.toggle_sidebar, "Ctrl+Alt+B");
    assert_eq!(config.shortcuts.toggle_hud, "Ctrl+Alt+U");
    assert_eq!(config.shortcuts.toggle_minimap, "Ctrl+Alt+M");
    assert_eq!(config.shortcuts.align_workspaces_horizontally, "Ctrl+Alt+A");
    assert_eq!(config.shortcuts.toggle_settings, "Ctrl+Alt+S");
    assert_eq!(config.shortcuts.zoom_reset, "Ctrl+Alt+0");
    assert_eq!(config.shortcuts.zoom_in, "Ctrl+Alt+Plus");
    assert_eq!(config.shortcuts.zoom_out, "Ctrl+Alt+Minus");
    assert_eq!(config.shortcuts.fullscreen_panel, "Ctrl+Alt+F");
    assert_eq!(config.shortcuts.exit_fullscreen_panel, "Ctrl+Alt+Escape");
    assert_eq!(config.shortcuts.fullscreen_window, "Ctrl+Alt+F11");
    assert_eq!(config.shortcuts.save_editor, "Ctrl+Alt+V");
    assert_eq!(config.shortcuts.search, "Ctrl+Alt+R");
}

#[test]
fn every_value_of_the_v027_file_with_non_default_sections_arrives() {
    let (_root, yaml, settings) = home_with("v0.2.7", "config-all-values.yaml");
    let before = fs::read(&yaml).expect("the file before the start");

    let config = Config::load_from_home(&settings).expect("the settings of v0.2.7");

    assert_window_look_and_shortcuts(&config, 1234.0, 777.0, 42.0, 24.0);
    assert_eq!(config.overlays.minimap_height, 200.0);
    assert_eq!(config.overlays.minimap_width, 400.0);

    let written = config.to_json().expect("the settings as JSON");
    for dropped in ["attention_feed", "speech", "desktop_injection"] {
        assert!(
            !written.contains(dropped),
            "{dropped} is a setting that this build removed, and the store holds it not: {written}"
        );
    }

    let names: Vec<&str> = config.presets.iter().map(|preset| preset.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Shell", "Build host", "Codex", "Markdown", "Grok", "Browser"],
        "the presets of the removed panels are gone, and the migrations add the two that came after \
         v0.2.7"
    );
    let shell = &config.presets[0];
    assert_eq!(shell.command.as_deref(), Some("/bin/zsh"));
    assert_eq!(shell.args, vec!["--login".to_string()]);
    let build = &config.presets[1];
    let connection = build.ssh_connection.as_ref().expect("the SSH preset keeps its host");
    assert_eq!(connection.host, "build.example.net");
    assert_eq!(connection.port, Some(2222));
    assert_eq!(connection.user.as_deref(), Some("builder"));
    assert_eq!(connection.identity_file.as_deref(), Some("/home/user/.ssh/build"));
    assert_eq!(connection.proxy_jump.as_deref(), Some("bastion.example.net"));
    assert_eq!(connection.remote_command.as_deref(), Some("tmux attach -t build"));
    assert_eq!(connection.extra_args, vec!["-o", "ServerAliveInterval=30"]);

    assert_eq!(config.workspaces.len(), 1);
    let workspace = &config.workspaces[0];
    assert_eq!(workspace.name, "Release checks");
    assert_eq!(workspace.color.as_deref(), Some("#3355ff"));
    assert_eq!(workspace.cwd.as_deref(), Some("/home/user/project"));
    assert_eq!(workspace.position, Some([120.0, 60.0]));
    let kinds: Vec<&str> = workspace
        .terminals
        .iter()
        .map(|terminal| match terminal.kind {
            horizon_core::PanelKind::Shell => "shell",
            horizon_core::PanelKind::Editor => "editor",
            other => panic!("a panel of {other:?} must not reach this build"),
        })
        .collect();
    assert_eq!(kinds, vec!["shell", "editor"]);
    let terminal = &workspace.terminals[0];
    assert_eq!(terminal.rows, 30);
    assert_eq!(terminal.cols, 100);
    assert_eq!(terminal.position, Some([120.0, 60.0]));
    assert_eq!(terminal.size, Some([640.0, 400.0]));
    assert_eq!(terminal.cwd.as_deref(), Some("/home/user/project"));

    assert_eq!(fs::read(&yaml).expect("the file after the start"), before);
}

#[test]
fn the_default_file_of_main_imports_with_every_value() {
    let (_root, yaml, settings) = home_with("main", "config.yaml");
    let before = fs::read(&yaml).expect("the file before the start");

    let config = Config::load_from_home(&settings).expect("the settings of main");

    assert_eq!(config.version, 12, "the migration of a version 11 file ran");
    assert_eq!(config.appearance.theme, AppearanceTheme::Auto);
    assert_eq!(config.window.width, 1600.0);
    assert_eq!(config.overlays.minimap_width, 320.0);
    assert!(!config.features.organize_workspaces_on_session_load);
    assert!(!config.features.sidebar_accordion);
    assert_eq!(
        config.remote_hosts.default_workspace,
        horizon_core::RemoteHostsConfig::DEFAULT_WORKSPACE
    );
    assert_eq!(config.remote_hosts.vnc_port, 5900);
    assert_eq!(config.browser_host.ssh_target, None);
    assert_eq!(config.browser_host.profile, None);
    assert!(config.workspaces.is_empty());
    assert_eq!(fs::read(&yaml).expect("the file after the start"), before);
}

#[test]
fn every_value_of_the_main_file_with_non_default_sections_arrives() {
    let (_root, yaml, settings) = home_with("main", "config-all-values.yaml");
    let before = fs::read(&yaml).expect("the file before the start");

    let config = Config::load_from_home(&settings).expect("the settings of main");

    assert_window_look_and_shortcuts(&config, 1440.0, 900.0, 12.0, 34.0);
    assert_eq!(config.overlays.minimap_height, 260.0);
    assert_eq!(config.overlays.minimap_width, 380.0);

    assert!(config.features.organize_workspaces_on_session_load);
    assert!(config.features.sidebar_accordion);
    let written = config.to_json().expect("the settings as JSON");
    for dropped in ["speech", "desktop_injection", "setup_board"] {
        assert!(
            !written.contains(dropped),
            "{dropped} is a setting that this build removed, and the store holds it not: {written}"
        );
    }

    assert_eq!(config.remote_hosts.default_workspace, "Remote Lab");
    assert_eq!(config.remote_hosts.vnc_port, 5901);
    assert_eq!(config.remote_hosts.vnc_ports.get("build.example.net"), Some(&5902));
    assert_eq!(config.remote_hosts.vnc_ports.get("lab"), Some(&5903));
    assert_eq!(
        config.browser_host.ssh_target.as_deref(),
        Some("browser-vm.example.net")
    );
    assert_eq!(config.browser_host.profile.as_deref(), Some("mail"));

    let names: Vec<&str> = config.presets.iter().map(|preset| preset.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Shell", "Build host", "Markdown", "Browser"],
        "the presets of the removed panels are gone and the others stay"
    );
    assert_eq!(config.presets[0].command.as_deref(), Some("/bin/zsh"));
    assert_eq!(
        config.presets[1].ssh_connection.as_ref().map(|ssh| ssh.host.as_str()),
        Some("build.example.net")
    );

    assert_eq!(config.workspaces.len(), 1);
    let workspace = &config.workspaces[0];
    assert_eq!(workspace.name, "Release checks");
    assert_eq!(workspace.cwd.as_deref(), Some("/home/user/project"));
    assert_eq!(workspace.position, Some([120.0, 60.0]));
    let kinds: Vec<&str> = workspace
        .terminals
        .iter()
        .map(|terminal| match terminal.kind {
            horizon_core::PanelKind::Shell => "shell",
            horizon_core::PanelKind::Editor => "editor",
            other => panic!("a panel of {other:?} must not reach this build"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["shell", "editor"],
        "the Usage, Git Changes and setup board panels leave the settings"
    );
    assert_eq!(workspace.terminals[0].rows, 30);
    assert_eq!(workspace.terminals[0].cols, 100);

    assert_eq!(fs::read(&yaml).expect("the file after the start"), before);
}

#[test]
fn every_browser_setting_of_the_main_file_arrives() {
    let (_root, _yaml, settings) = home_with("main", "config-all-values.yaml");

    let config = Config::load_from_home(&settings).expect("the settings of main");
    let written = config.to_json().expect("the settings as JSON");

    let browser = &config.browser;
    assert_eq!(browser.backend, horizon_core::browser::BackendKind::FirefoxBidi);
    assert!(!browser.headless);
    assert!(browser.hide_native_window);
    assert!(browser.firefox_system_access);
    assert_eq!(browser.command.as_deref(), Some("/usr/bin/chromium"));
    assert_eq!(browser.firefox_command.as_deref(), Some("/usr/bin/firefox"));
    assert_eq!(browser.geckodriver_command.as_deref(), Some("/usr/bin/geckodriver"));
    assert_eq!(browser.safaridriver_command, None);
    assert_eq!(browser.extra_args, vec!["--disable-features=Translate".to_string()]);
    assert_eq!(browser.quality, 75);
    assert_eq!(browser.every_nth_frame, 2);
    assert_eq!(
        browser.profile_root.as_deref(),
        Some(Path::new("/home/user/browser-profiles"))
    );
    assert_eq!(
        browser.downloads_root.as_deref(),
        Some(Path::new("/home/user/Downloads"))
    );
    assert_eq!(browser.video.quality, 70);
    assert_eq!(browser.video.compression_level, 6);
    assert_eq!(browser.video.fps, 15);
    assert_eq!(browser.video.max_width, Some(1600));
    assert_eq!(browser.video.max_file_bytes, 104_857_600);
    assert_eq!(browser.webauthn_command, vec!["/usr/bin/authenticator".to_string()]);
    assert_eq!(browser.remote.providers.len(), 1);
    assert_eq!(browser.remote.targets.len(), 1);
    assert_eq!(
        browser.remote.providers["device_grid"].credential_bindings.len(),
        2,
        "a binding of the credential store of the operating system reads as a binding of the \
         secrets file, whose value the person types once more"
    );
    assert!(
        written.contains("\"secrets_file\""),
        "the store holds the token of this build, not the one of the removed OS store: {written}"
    );
    assert!(!written.contains("os_keychain"), "{written}");
}

#[test]
fn the_second_start_of_an_imported_home_reads_the_store_only() {
    let (_root, yaml, settings) = home_with("v0.2.7", "config-all-values.yaml");
    Config::load_from_home(&settings).expect("the import of the YAML file");
    fs::remove_file(&yaml).expect("the YAML file leaves the home");

    let config = Config::load_from_home(&settings).expect("the second start");

    assert_eq!(config.appearance.theme, AppearanceTheme::Light);
    assert_eq!(config.window.width, 1234.0);
    assert_eq!(config.shortcuts.search, "Ctrl+Alt+R");
    assert_eq!(config.workspaces.len(), 1);
}

#[test]
fn a_yaml_file_that_cannot_be_read_writes_no_store_and_names_its_line() {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = root.path().join(".horizon");
    fs::create_dir_all(&home).expect("the Horizon home");
    let yaml = home.join("config.yaml");
    let broken = "version: 8\nappearance:\n  theme: dark\n   theme: light\n";
    fs::write(&yaml, broken).expect("the broken settings file");
    let settings = home.join("config.json");

    let error = Config::load_from_home(&settings).expect_err("a broken file stops the start");

    let message = error.to_string();
    assert!(message.contains(&yaml.display().to_string()), "{message}");
    assert!(message.contains("line"), "{message}");
    assert!(!ConfigStore::resolve(&settings).path().exists(), "no store is written");
    assert_eq!(
        fs::read_to_string(&yaml).expect("the broken file"),
        broken,
        "the broken file keeps every byte"
    );
}
