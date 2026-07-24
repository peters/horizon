use std::path::{Path, PathBuf};
use std::{env, fs, io};

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

fn main() -> io::Result<()> {
    emit_cuda_runtime_link_workaround();

    let manifest_dir = required_path_var("CARGO_MANIFEST_DIR")?;
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

/// Workaround for transcribe-cpp-sys 0.1.3: its `transcribe-link.json`
/// manifest omits the CUDA runtime libraries for `TRANSCRIBE_CUDA` builds
/// (`system_libs` carries only stdc++/m/pthread/dl), so every downstream
/// bin/test target fails to link with undefined `cuda*` symbols. Emit them
/// here until a -sys release records the CUDA deps in the manifest itself.
fn emit_cuda_runtime_link_workaround() {
    if env::var_os("CARGO_FEATURE_SPEECH_CUDA").is_none() {
        return;
    }
    let cuda_root = env::var("CUDA_PATH")
        .or_else(|_| env::var("CUDA_HOME"))
        .unwrap_or_else(|_| "/usr/local/cuda".to_string());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Windows CUDA toolkits ship import libraries under lib/x64.
        println!("cargo:rustc-link-search=native={cuda_root}/lib/x64");
    } else {
        println!("cargo:rustc-link-search=native={cuda_root}/lib64");
        // libcuda.so (driver API) resolves from the stubs dir at link time
        // and from the installed driver at runtime.
        println!("cargo:rustc-link-search=native={cuda_root}/lib64/stubs");
    }
    for lib in ["cudart", "cublas", "cublasLt", "cuda"] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");
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
