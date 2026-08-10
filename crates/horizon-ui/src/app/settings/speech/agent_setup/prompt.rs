//! One-shot prompt for the fresh Speech Input setup agent panel.

use std::path::{Path, PathBuf};

use super::SpeechSetupAgent;

pub(in crate::app) fn speech_setup_prompt(agent: SpeechSetupAgent, config_path: &Path) -> String {
    let executable = std::env::current_exe().map_or_else(
        |error| format!("<could not resolve current executable: {error}>"),
        |path| quoted_path_for_prompt(&path),
    );
    let config_path = config_path_for_prompt(config_path, std::env::current_dir());
    let agent_name = agent.display_name();

    format!(
        r"Help me set up Horizon Speech Input on this machine. This is a new, one-shot {agent_name} setup session; do not resume or rely on an earlier conversation.

Safety and first inspection
1. Begin read-only. Inspect the OS, CPU/GPU and usable inference backends, the currently running Horizon executable, Horizon's config path, the active working directory and any available Horizon source checkout, microphone devices, required build prerequisites, and existing local GGUF speech models. Do not install, download, replace, restart, or edit anything during this first pass.
2. The running executable reported by Horizon is {executable}. The config path reported by Horizon is {config_path}. Verify both rather than assuming they are authoritative, and identify whether this is a source build or a managed installation.
3. If a Horizon checkout is available, read its AGENTS.md instructions and the current Speech Input section of README.md before proposing changes. Preserve unrelated configuration and all tracked or untracked source changes.

Decisions and approvals
4. Ask which spoken language or languages I need and whether I prefer speed or accuracy before choosing a model. Configure one working starter profile first; do not introduce multi-profile complexity unless I ask for it.
5. Ask for explicit approval before installing packages, downloading a large model, replacing any Horizon binary, or restarting Horizon. Treat managed installations carefully and never overwrite their managed binary without explicit approval.
6. Before any GGUF download, show the proposed model's source URL or repository, license, download size, destination path, supported spoken languages, translation support and targets, and expected RAM/VRAM use. Audio processing must remain local after the model has been downloaded.

Implementation
7. Prefer `backend: auto` unless inspection establishes a reason not to. After loading the model, report the backend that actually loaded rather than merely repeating the configured value.
8. Back up the existing Horizon config, preserve unrelated keys and presets, validate the complete result, and replace the config atomically. Horizon automatically reloads valid external config changes after Settings closes.
9. If speech support is absent, build the appropriate existing speech feature for this machine (for example `speech`, `speech-cuda`, or `speech-vulkan`) and follow the checkout's prerequisites and validation guidance. Do not modify Horizon source merely to make setup pass.
10. Keep the running parent application boundary in mind: a newly built or replaced binary cannot add speech support to the already-running process. Explain the exact relaunch needed and ask before replacing a binary or restarting the application.

Verification and handoff
11. Verify the exact built or selected Horizon binary, successful GGUF loading, the selected microphone, and an inert dictation test as far as the running parent application allows. Do not inject a real transcription into an unrelated terminal or application as a test.
12. Finish with an exact summary of files and configuration changed, model provenance and destination, the backend actually loaded, verification performed, any required relaunch command, and remaining limitations. Before finishing, send a Horizon completion notification through the available horizon-notify integration when HORIZON is set; if that integration is unavailable, say so explicitly.

Pause and ask whenever the next step needs approval or the evidence is ambiguous."
    )
}

fn config_path_for_prompt(config_path: &Path, process_cwd: std::io::Result<PathBuf>) -> String {
    if config_path.is_absolute() {
        return quoted_path_for_prompt(config_path);
    }

    process_cwd.map_or_else(
        |error| format!("<could not resolve config path from process working directory: {error}>"),
        |process_cwd| quoted_path_for_prompt(&absolute_config_path_from(config_path, &process_cwd)),
    )
}

fn quoted_path_for_prompt(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => quoted.extend(character.escape_default()),
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn absolute_config_path_from(config_path: &Path, process_cwd: &Path) -> PathBuf {
    if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        process_cwd.join(config_path)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{absolute_config_path_from, config_path_for_prompt, quoted_path_for_prompt, speech_setup_prompt};
    use crate::app::settings::speech::agent_setup::SpeechSetupAgent;

    #[test]
    fn prompt_contains_every_setup_safety_and_handoff_requirement() {
        let prompt = speech_setup_prompt(SpeechSetupAgent::Codex, Path::new("/tmp/horizon config.yaml"));

        for required in [
            "Begin read-only",
            "OS, CPU/GPU",
            "currently running Horizon executable",
            "config path",
            "Horizon source checkout",
            "microphone devices",
            "build prerequisites",
            "local GGUF",
            "AGENTS.md",
            "Speech Input section of README.md",
            "Preserve unrelated configuration",
            "tracked or untracked source changes",
            "spoken language",
            "speed or accuracy",
            "one working starter profile",
            "explicit approval before installing packages",
            "downloading a large model",
            "replacing any Horizon binary",
            "restarting Horizon",
            "managed installations",
            "source URL or repository",
            "license",
            "download size",
            "destination path",
            "supported spoken languages",
            "translation support",
            "expected RAM/VRAM use",
            "Audio processing must remain local",
            "backend: auto",
            "backend that actually loaded",
            "Back up the existing Horizon config",
            "replace the config atomically",
            "speech-cuda",
            "speech-vulkan",
            "Do not modify Horizon source merely to make setup pass",
            "managed binary",
            "exact built or selected Horizon binary",
            "successful GGUF loading",
            "selected microphone",
            "inert dictation test",
            "exact relaunch",
            "remaining limitations",
            "Horizon completion notification",
            "horizon-notify",
        ] {
            assert!(prompt.contains(required), "prompt is missing `{required}`");
        }
    }

    #[test]
    fn prompt_names_agent_and_safely_quotes_the_config_path() {
        let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("lyd-æøå\"\\\nconfig.yaml");
        let prompt = speech_setup_prompt(SpeechSetupAgent::Claude, &config_path);
        assert!(prompt.contains("one-shot Claude setup session"));
        assert!(prompt.contains(&quoted_path_for_prompt(&config_path)));
        assert!(prompt.contains("lyd-æøå\\\"\\\\\\nconfig.yaml"));
        assert!(!prompt.contains("lyd-æøå\"\\\nconfig.yaml"));
    }

    #[test]
    fn relative_config_path_is_resolved_from_process_launch_directory() {
        let process_cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert!(process_cwd.is_absolute());

        let relative = Path::new("runtime/config.yaml");
        let resolved = absolute_config_path_from(relative, &process_cwd);
        let rendered = config_path_for_prompt(relative, Ok(process_cwd.clone()));

        assert_eq!(resolved, process_cwd.join("runtime/config.yaml"));
        assert!(resolved.is_absolute());
        assert_eq!(rendered, quoted_path_for_prompt(&resolved));
    }

    #[test]
    fn absolute_config_path_does_not_depend_on_process_launch_directory() {
        let process_cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let absolute = process_cwd.join("config.yaml");

        assert_eq!(absolute_config_path_from(&absolute, Path::new("ignored")), absolute);
        assert_eq!(
            config_path_for_prompt(&absolute, Err(io::Error::other("cwd unavailable"))),
            quoted_path_for_prompt(&absolute)
        );
    }
}
