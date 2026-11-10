use std::path::{Path, PathBuf};
use std::{env, fs, io};

#[path = "build/font_assets.rs"]
mod font_assets;

/// Assets that live under the workspace-root `assets/` directory and are
/// embedded via `include_bytes!`/`include_str!` in the crate source.
///
/// During a normal workspace build the files are found at
/// `CARGO_MANIFEST_DIR/../../assets/<relative_path>`.  During `cargo publish`
/// verification the workspace root is not available, so CI copies the files
/// into `CARGO_MANIFEST_DIR/publish-assets/` beforehand.
const ASSETS: &[&str] = &[
    "icons/icon-64.png",
    "icons/icon-128.png",
    "icons/icon-256.png",
    "icons/icon-512.png",
    "icons/logo.svg",
    "plugins/claude-code/.claude-plugin/plugin.json",
    "plugins/claude-code/skills/horizon-browser/SKILL.md",
    "plugins/claude-code/skills/horizon-device/SKILL.md",
    "plugins/codex/skills/horizon-browser/SKILL.md",
    "plugins/codex/skills/horizon-device/SKILL.md",
];

fn main() -> io::Result<()> {
    emit_macos_swift_runtime_rpath();

    let manifest_dir = required_path_var("CARGO_MANIFEST_DIR")?;
    font_assets::validate(&manifest_dir)?;
    let out_dir = required_path_var("OUT_DIR")?;

    // Candidate roots, checked in order.
    let publish_assets = manifest_dir.join("publish-assets");
    let workspace_assets = manifest_dir.join("../../assets");

    for relative in ASSETS {
        let src = resolve_asset(relative, &publish_assets, &workspace_assets)?;

        let dest = out_dir.join("assets").join(relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dest)?;

        println!("cargo:rerun-if-changed={}", src.display());
    }

    Ok(())
}

/// Ensure the final Horizon binary can find the OS Swift runtime even when a
/// workflow-level `RUSTFLAGS` overrides target flags from Cargo configuration.
fn emit_macos_swift_runtime_rpath() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}

fn required_path_var(name: &'static str) -> io::Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} not set")))
}

fn resolve_asset(relative: &str, publish_assets: &Path, workspace_assets: &Path) -> io::Result<PathBuf> {
    let publish_path = publish_assets.join(relative);
    if publish_path.exists() {
        return Ok(publish_path);
    }

    let workspace_path = workspace_assets.join(relative);
    if workspace_path.exists() {
        return Ok(workspace_path);
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "asset {relative} not found in either:\n  1. {}\n  2. {}\n\
             During `cargo publish` verification, run the CI 'Stage assets for publish' step first.",
            publish_path.display(),
            workspace_path.display(),
        ),
    ))
}
