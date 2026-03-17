use std::path::{Path, PathBuf};
use std::{env, fs};

/// Assets that live under the workspace-root `assets/` directory and are
/// embedded via `include_bytes!`/`include_str!` in the crate source.
///
/// During a normal workspace build the files are found at
/// `CARGO_MANIFEST_DIR/../../assets/<relative_path>`.  During `cargo publish`
/// verification the workspace root is not available, so CI copies the files
/// into `CARGO_MANIFEST_DIR/publish-assets/` beforehand.
const ASSETS: &[&str] = &[
    "icons/icon-128.png",
    "plugins/claude-code/.claude-plugin/plugin.json",
    "plugins/claude-code/skills/horizon-notify/SKILL.md",
    "plugins/codex/skills/horizon-notify/SKILL.md",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));

    // Candidate roots, checked in order.
    let publish_assets = manifest_dir.join("publish-assets");
    let workspace_assets = manifest_dir.join("../../assets");

    for relative in ASSETS {
        let src = resolve_asset(relative, &publish_assets, &workspace_assets);

        let dest = out_dir.join("assets").join(relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("failed to create {}: {e}", parent.display()));
        }
        fs::copy(&src, &dest).unwrap_or_else(|e| {
            panic!(
                "failed to copy {} -> {}: {e}",
                src.display(),
                dest.display()
            )
        });

        println!("cargo:rerun-if-changed={}", src.display());
    }
}

fn resolve_asset(relative: &str, publish_assets: &Path, workspace_assets: &Path) -> PathBuf {
    let publish_path = publish_assets.join(relative);
    if publish_path.exists() {
        return publish_path;
    }

    let workspace_path = workspace_assets.join(relative);
    if workspace_path.exists() {
        return workspace_path;
    }

    panic!(
        "asset {relative} not found in either:\n  1. {}\n  2. {}\n\
         During `cargo publish` verification, run the CI 'Stage assets for publish' step first.",
        publish_path.display(),
        workspace_path.display(),
    );
}
