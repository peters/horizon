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
    "plugins/claude-code/skills/horizon-block/SKILL.md",
    "plugins/claude-code/skills/horizon-device/SKILL.md",
    "plugins/claude-code/skills/horizon-machines/SKILL.md",
    "plugins/claude-code/skills/horizon-machines/references/aws.md",
    "plugins/claude-code/skills/horizon-machines/references/browser-machine.md",
    "plugins/claude-code/skills/horizon-machines/references/browser-machine/Dockerfile",
    "plugins/claude-code/skills/horizon-machines/references/browser-machine/entrypoint.sh",
    "plugins/claude-code/skills/horizon-machines/references/browser-machine/horizon-browser-wrapper.sh",
    "plugins/claude-code/skills/horizon-machines/references/browser-machine/setup.sh",
    "plugins/claude-code/skills/horizon-machines/references/azure.md",
    "plugins/claude-code/skills/horizon-machines/references/gcp.md",
    "plugins/claude-code/skills/horizon-machines/references/hetzner.md",
    "plugins/claude-code/skills/horizon-machines/references/repository-machine.md",
    "plugins/claude-code/skills/horizon-machines/references/runpod.md",
    "plugins/claude-code/skills/horizon-machines/references/tailscale.md",
    "plugins/claude-code/skills/horizon-mobile/SKILL.md",
    "plugins/claude-code/skills/horizon-mobile/references/android.md",
    "plugins/claude-code/skills/horizon-mobile/references/ios.md",
    "plugins/claude-code/skills/horizon-mobile/references/provider.md",
    "plugins/claude-code/skills/horizon-mobile/scripts/mobile_session.py",
    "plugins/claude-code/skills/horizon-plan/SKILL.md",
    "plugins/claude-code/skills/horizon-server/SKILL.md",
    "plugins/claude-code/skills/horizon-server-move/SKILL.md",
    "plugins/claude-code/skills/horizon-setup/SKILL.md",
    "plugins/claude-code/skills/horizon-speech/SKILL.md",
    "plugins/claude-code/skills/horizon-worktree/SKILL.md",
    "plugins/codex/skills/horizon-browser/SKILL.md",
    "plugins/codex/skills/horizon-block/SKILL.md",
    "plugins/codex/skills/horizon-device/SKILL.md",
    "plugins/codex/skills/horizon-machines/SKILL.md",
    "plugins/codex/skills/horizon-machines/references/aws.md",
    "plugins/codex/skills/horizon-machines/references/browser-machine.md",
    "plugins/codex/skills/horizon-machines/references/browser-machine/Dockerfile",
    "plugins/codex/skills/horizon-machines/references/browser-machine/entrypoint.sh",
    "plugins/codex/skills/horizon-machines/references/browser-machine/horizon-browser-wrapper.sh",
    "plugins/codex/skills/horizon-machines/references/browser-machine/setup.sh",
    "plugins/codex/skills/horizon-machines/references/azure.md",
    "plugins/codex/skills/horizon-machines/references/gcp.md",
    "plugins/codex/skills/horizon-machines/references/hetzner.md",
    "plugins/codex/skills/horizon-machines/references/repository-machine.md",
    "plugins/codex/skills/horizon-machines/references/runpod.md",
    "plugins/codex/skills/horizon-machines/references/tailscale.md",
    "plugins/codex/skills/horizon-mobile/SKILL.md",
    "plugins/codex/skills/horizon-mobile/references/android.md",
    "plugins/codex/skills/horizon-mobile/references/ios.md",
    "plugins/codex/skills/horizon-mobile/references/provider.md",
    "plugins/codex/skills/horizon-mobile/scripts/mobile_session.py",
    "plugins/codex/skills/horizon-plan/SKILL.md",
    "plugins/codex/skills/horizon-server/SKILL.md",
    "plugins/codex/skills/horizon-server-move/SKILL.md",
    "plugins/codex/skills/horizon-setup/SKILL.md",
    "plugins/codex/skills/horizon-speech/SKILL.md",
    "plugins/codex/skills/horizon-worktree/SKILL.md",
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
