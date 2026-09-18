//! Install a user-level `.desktop` entry and hicolor icons so Linux docks can
//! match a `cargo run` / tarball window to Horizon's icon.
//!
//! Ubuntu Dock looks up `Icon=` from a desktop file whose id or
//! `StartupWMClass` matches `WM_CLASS`. The window already sets `_NET_WM_ICON`;
//! GNOME's dock does not use that property for unmatched apps.

use std::io::{self, Write};
use std::path::Path;

use crate::branding;

const DESKTOP_FILE_NAME: &str = "horizon.desktop";
const GENERATED_MARKER: &str = "X-Horizon-Desktop=generated";
const STOCK_PACKAGING_DESKTOP: &str = "\
[Desktop Entry]
Type=Application
Version=1.0
Name=Horizon
GenericName=Spatial Workspace
Comment=Your work, one canvas
Exec=horizon
Icon=horizon
Terminal=false
Categories=System;TerminalEmulator;
StartupNotify=true
StartupWMClass=horizon
";

const ICON_PNGS: &[(u32, &[u8])] = &[
    (
        64,
        include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/icon-64.png")),
    ),
    (
        128,
        include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/icon-128.png")),
    ),
    (
        256,
        include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/icon-256.png")),
    ),
    (
        512,
        include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/icon-512.png")),
    ),
];

const ICON_SVG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/assets/icons/logo.svg"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    Snap,
    Flatpak,
    ManagedInstall,
    PackagedDesktop,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snap => formatter.write_str("snap"),
            Self::Flatpak => formatter.write_str("flatpak"),
            Self::ManagedInstall => formatter.write_str("managed install"),
            Self::PackagedDesktop => formatter.write_str("packaged desktop file"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallChanges {
    desktop: bool,
    icons: usize,
}

#[cfg(target_os = "linux")]
pub(crate) fn install() {
    match install_from_environment() {
        Ok(None) => {}
        Ok(Some(reason)) => {
            tracing::debug!(%reason, "skipping Linux desktop entry install");
        }
        Err(error) => {
            tracing::warn!(%error, "could not install Linux desktop entry");
        }
    }
}

#[cfg(target_os = "linux")]
fn install_from_environment() -> io::Result<Option<SkipReason>> {
    let exe = std::env::current_exe()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if let Some(reason) = skip_reason(
        std::env::var_os("SNAP").is_some().then_some(SkipReason::Snap),
        std::env::var_os("FLATPAK_ID").is_some().then_some(SkipReason::Flatpak),
        horizon_core::ManagedInstall::discover(&exe)
            .is_some()
            .then_some(SkipReason::ManagedInstall),
        packaged_desktop_exists().then_some(SkipReason::PackagedDesktop),
    ) {
        return Ok(Some(reason));
    }

    let data_home = xdg_data_home()?;
    let changes = install_into(&exe, &data_home)?;
    if changes.desktop || changes.icons > 0 {
        tracing::info!(
            desktop = changes.desktop,
            icons = changes.icons,
            path = %data_home.join("applications").join(DESKTOP_FILE_NAME).display(),
            "installed Linux desktop entry"
        );
    }
    Ok(None)
}

fn skip_reason(
    snap: Option<SkipReason>,
    flatpak: Option<SkipReason>,
    managed: Option<SkipReason>,
    packaged: Option<SkipReason>,
) -> Option<SkipReason> {
    snap.or(flatpak).or(managed).or(packaged)
}

#[cfg(target_os = "linux")]
fn packaged_desktop_exists() -> bool {
    packaged_data_dirs(std::env::var_os("XDG_DATA_DIRS").as_deref())
        .iter()
        .any(|dir| dir.join("applications").join(DESKTOP_FILE_NAME).exists())
}

fn packaged_data_dirs(xdg_data_dirs: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    let parsed = xdg_data_dirs
        .filter(|value| !value.is_empty())
        .map(|value| {
            std::env::split_paths(value)
                .filter(|path| path.is_absolute())
                .collect::<Vec<_>>()
        })
        .filter(|dirs: &Vec<_>| !dirs.is_empty());
    parsed.unwrap_or_else(|| {
        vec![
            std::path::PathBuf::from("/usr/local/share"),
            std::path::PathBuf::from("/usr/share"),
        ]
    })
}

#[cfg(target_os = "linux")]
fn xdg_data_home() -> io::Result<std::path::PathBuf> {
    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        let path = std::path::PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    let home =
        horizon_core::user_home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(home.join(".local/share"))
}

fn install_into(exe: &Path, data_home: &Path) -> io::Result<InstallChanges> {
    let icons = install_icons(data_home)?;
    let desktop = install_desktop_file(exe, data_home)?;
    Ok(InstallChanges { desktop, icons })
}

fn install_icons(data_home: &Path) -> io::Result<usize> {
    let mut updated = 0usize;
    let hicolor = data_home.join("icons/hicolor");
    for &(size, bytes) in ICON_PNGS {
        let path = hicolor.join(format!("{size}x{size}/apps/horizon.png"));
        if sync_bytes_if_changed(&path, bytes)? {
            updated += 1;
        }
    }
    let svg = hicolor.join("scalable/apps/horizon.svg");
    if sync_bytes_if_changed(&svg, ICON_SVG)? {
        updated += 1;
    }
    Ok(updated)
}

fn install_desktop_file(exe: &Path, data_home: &Path) -> io::Result<bool> {
    let path = data_home.join("applications").join(DESKTOP_FILE_NAME);
    let contents = desktop_file_contents(exe)?;
    if let Ok(existing) = std::fs::read_to_string(&path)
        && !should_replace_desktop(&existing)
    {
        return Ok(false);
    }
    sync_bytes_if_changed(&path, contents.as_bytes())
}

fn should_replace_desktop(existing: &str) -> bool {
    existing.contains(GENERATED_MARKER) || normalize_desktop(existing) == normalize_desktop(STOCK_PACKAGING_DESKTOP)
}

fn normalize_desktop(contents: &str) -> String {
    contents.trim().replace("\r\n", "\n")
}

fn desktop_file_contents(exe: &Path) -> io::Result<String> {
    let exec = quote_exec(exe)?;
    Ok(format!(
        "\
[Desktop Entry]
Type=Application
Version=1.0
Name={name}
GenericName=Spatial Workspace
Comment=Your work, one canvas
Exec={exec}
Icon={id}
Terminal=false
Categories=System;TerminalEmulator;
StartupNotify=true
StartupWMClass={id}
{GENERATED_MARKER}
",
        name = branding::APP_NAME,
        id = branding::APP_ID,
    ))
}

fn quote_exec(path: &Path) -> io::Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "executable path is not valid UTF-8"))?;
    let value = value.replace('%', "%%");
    if value.bytes().any(|byte| {
        matches!(
            byte,
            b' ' | b'\t'
                | b'"'
                | b'\\'
                | b'\''
                | b'>'
                | b'<'
                | b'~'
                | b'|'
                | b'&'
                | b';'
                | b'$'
                | b'*'
                | b'?'
                | b'#'
                | b'('
                | b')'
                | b'`'
        )
    }) {
        Ok(format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
                .replace('`', "\\`")
        ))
    } else {
        Ok(value)
    }
}

fn sync_bytes_if_changed(path: &Path, content: &[u8]) -> io::Result<bool> {
    if std::fs::read(path).ok().as_deref() == Some(content) {
        return Ok(false);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content)?;
    temp_file.flush()?;
    temp_file.persist(path).map_err(|error| error.error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{
        DESKTOP_FILE_NAME, GENERATED_MARKER, ICON_PNGS, STOCK_PACKAGING_DESKTOP, SkipReason, desktop_file_contents,
        install_into, normalize_desktop, packaged_data_dirs, quote_exec, should_replace_desktop, skip_reason,
    };
    use std::path::Path;

    struct DataHome {
        _guard: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    fn temp_data_home() -> DataHome {
        let guard = tempfile::TempDir::new().expect("temp data home");
        let path = guard.path().join("share");
        DataHome { _guard: guard, path }
    }

    #[test]
    fn skip_reason_labels_are_stable() {
        assert_eq!(SkipReason::Snap.to_string(), "snap");
        assert_eq!(SkipReason::Flatpak.to_string(), "flatpak");
        assert_eq!(SkipReason::ManagedInstall.to_string(), "managed install");
        assert_eq!(SkipReason::PackagedDesktop.to_string(), "packaged desktop file");
    }

    #[test]
    fn skip_reason_prefers_packaged_installs() {
        assert_eq!(
            skip_reason(Some(SkipReason::Snap), Some(SkipReason::Flatpak), None, None),
            Some(SkipReason::Snap)
        );
        assert_eq!(
            skip_reason(None, Some(SkipReason::Flatpak), Some(SkipReason::ManagedInstall), None),
            Some(SkipReason::Flatpak)
        );
        assert_eq!(
            skip_reason(
                None,
                None,
                Some(SkipReason::ManagedInstall),
                Some(SkipReason::PackagedDesktop)
            ),
            Some(SkipReason::ManagedInstall)
        );
        assert_eq!(
            skip_reason(None, None, None, Some(SkipReason::PackagedDesktop)),
            Some(SkipReason::PackagedDesktop)
        );
        assert_eq!(skip_reason(None, None, None, None), None);
    }

    #[test]
    fn quote_exec_quotes_reserved_characters() {
        assert_eq!(
            quote_exec(Path::new("/opt/horizon/horizon")).expect("plain path"),
            "/opt/horizon/horizon"
        );
        assert_eq!(
            quote_exec(Path::new("/opt/Horizon App/horizon")).expect("spaced path"),
            "\"/opt/Horizon App/horizon\""
        );
        assert_eq!(
            quote_exec(Path::new("/opt/100%/horizon")).expect("percent path"),
            "/opt/100%%/horizon"
        );
        assert_eq!(
            quote_exec(Path::new("/opt/foo$/horizon")).expect("dollar path"),
            "\"/opt/foo\\$/horizon\""
        );
    }

    #[test]
    fn packaged_data_dirs_use_xdg_data_dirs_then_defaults() {
        assert_eq!(
            packaged_data_dirs(None),
            vec![
                std::path::PathBuf::from("/usr/local/share"),
                std::path::PathBuf::from("/usr/share")
            ]
        );
        let custom = std::env::join_paths(["/opt/share", "/custom/share"]).expect("join paths");
        assert_eq!(
            packaged_data_dirs(Some(custom.as_os_str())),
            vec![
                std::path::PathBuf::from("/opt/share"),
                std::path::PathBuf::from("/custom/share")
            ]
        );
        assert_eq!(
            packaged_data_dirs(Some(std::ffi::OsStr::new("relative"))),
            vec![
                std::path::PathBuf::from("/usr/local/share"),
                std::path::PathBuf::from("/usr/share")
            ]
        );
    }

    #[test]
    fn stock_desktop_matches_packaging_file() {
        let packaged = include_str!("../../../packaging/linux/horizon.desktop");
        assert_eq!(normalize_desktop(STOCK_PACKAGING_DESKTOP), normalize_desktop(packaged));
    }

    #[test]
    fn generated_and_stock_desktop_files_are_replaceable() {
        let generated = desktop_file_contents(Path::new("/opt/horizon/horizon")).expect("desktop contents");
        assert!(generated.contains(GENERATED_MARKER));
        assert!(generated.contains("StartupWMClass=horizon"));
        assert!(generated.contains("Exec=/opt/horizon/horizon"));
        assert!(should_replace_desktop(&generated));
        assert!(should_replace_desktop(STOCK_PACKAGING_DESKTOP));
        assert!(!should_replace_desktop(
            "[Desktop Entry]\nName=Custom Horizon\nExec=/opt/custom\nStartupWMClass=horizon\n"
        ));
    }

    #[test]
    fn install_into_writes_icons_and_desktop_file() {
        let data_home = temp_data_home();
        let exe = Path::new("/home/peters/github/horizon/target/release/horizon");
        let first = install_into(exe, &data_home.path).expect("install");
        assert!(first.desktop);
        assert_eq!(first.icons, ICON_PNGS.len() + 1);

        let desktop =
            std::fs::read_to_string(data_home.path.join("applications").join(DESKTOP_FILE_NAME)).expect("desktop");
        assert!(desktop.contains("Exec=/home/peters/github/horizon/target/release/horizon"));
        assert!(desktop.contains(GENERATED_MARKER));
        assert!(data_home.path.join("icons/hicolor/128x128/apps/horizon.png").is_file());
        assert!(data_home.path.join("icons/hicolor/scalable/apps/horizon.svg").is_file());

        let second = install_into(exe, &data_home.path).expect("reinstall");
        assert!(!second.desktop);
        assert_eq!(second.icons, 0);
    }

    #[test]
    fn install_into_does_not_overwrite_custom_desktop_file() {
        let data_home = temp_data_home();
        let applications = data_home.path.join("applications");
        std::fs::create_dir_all(&applications).expect("applications dir");
        let desktop_path = applications.join(DESKTOP_FILE_NAME);
        std::fs::write(&desktop_path, "[Desktop Entry]\nName=My Horizon\nExec=/opt/custom\n").expect("custom desktop");

        let changes = install_into(Path::new("/opt/horizon/horizon"), &data_home.path).expect("install");
        assert!(!changes.desktop);
        assert!(changes.icons > 0);
        let desktop = std::fs::read_to_string(&desktop_path).expect("desktop");
        assert!(desktop.contains("Name=My Horizon"));
    }

    #[test]
    fn install_into_replaces_stock_packaging_desktop_file() {
        let data_home = temp_data_home();
        let applications = data_home.path.join("applications");
        std::fs::create_dir_all(&applications).expect("applications dir");
        std::fs::write(applications.join(DESKTOP_FILE_NAME), STOCK_PACKAGING_DESKTOP).expect("stock desktop");

        let changes = install_into(Path::new("/opt/horizon/horizon"), &data_home.path).expect("install");
        assert!(changes.desktop);
        let desktop = std::fs::read_to_string(applications.join(DESKTOP_FILE_NAME)).expect("desktop");
        assert!(desktop.contains("Exec=/opt/horizon/horizon"));
        assert!(desktop.contains(GENERATED_MARKER));
    }
}
