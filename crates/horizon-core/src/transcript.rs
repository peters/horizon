use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::OnceLock;
#[cfg(not(windows))]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::panel::PanelKind;

const TRANSCRIPT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const TRANSCRIPT_COMPACT_SLACK_BYTES: u64 = 512 * 1024;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const USES_BSD_SCRIPT: bool = true;
#[cfg(not(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
const USES_BSD_SCRIPT: bool = false;

static SCRIPT_SUPPORT: OnceLock<ScriptSupport> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptFlavor {
    Bsd,
    UtilLinux,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScriptSupport {
    Missing,
    Unsupported { reason: String },
    Supported { program: String, flavor: ScriptFlavor },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelTranscript {
    root: PathBuf,
    local_id: String,
}

impl PanelTranscript {
    #[must_use]
    pub fn for_panel(kind: PanelKind, root: Option<PathBuf>, local_id: &str) -> Option<Self> {
        if !matches!(kind, PanelKind::Shell | PanelKind::Ssh | PanelKind::Command) {
            return None;
        }

        root.map(|root| Self {
            root,
            local_id: local_id.to_string(),
        })
    }

    #[must_use]
    pub fn history_path(&self) -> PathBuf {
        self.root.join(format!("{}.bin", self.local_id))
    }

    #[must_use]
    pub fn session_path(&self) -> PathBuf {
        self.root.join(format!("{}.session", self.local_id))
    }

    #[must_use]
    pub(crate) fn has_persisted_state(&self) -> bool {
        self.history_path().exists() || self.session_path().exists()
    }

    #[must_use]
    pub fn wrap_launch_command(&self, program: String, args: Vec<String>) -> (String, Vec<String>) {
        wrap_launch_command_with_support(script_support(), &self.session_path(), program, args)
    }

    /// Finalize any previous in-flight capture and return the retained replay bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the transcript files cannot be read or compacted.
    pub fn prepare_replay_bytes(&self) -> Result<Vec<u8>> {
        fs::create_dir_all(&self.root)?;
        self.finalize_live_session()?;
        let mut bytes = read_tail(&self.history_path(), TRANSCRIPT_MAX_BYTES)?;
        ensure_clean_prompt_boundary(&mut bytes);
        Ok(bytes)
    }

    /// Delete all persisted transcript state for this panel.
    ///
    /// # Errors
    ///
    /// Returns an error if transcript files exist but cannot be removed.
    pub fn delete_all(&self) -> Result<()> {
        remove_if_exists(&self.history_path())?;
        remove_if_exists(&self.session_path())?;
        Ok(())
    }

    fn finalize_live_session(&self) -> Result<()> {
        let session_path = self.session_path();
        if !session_path.exists() {
            return Ok(());
        }

        let bytes = fs::read(&session_path)?;
        let sanitized = strip_script_banners(&bytes);
        if !sanitized.is_empty() {
            append_and_compact(&self.history_path(), &sanitized)?;
        }

        remove_if_exists(&session_path)?;
        Ok(())
    }
}

fn append_and_compact(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(bytes)?;
    file.flush()?;

    let size = file.metadata()?.len();
    if size > TRANSCRIPT_MAX_BYTES + TRANSCRIPT_COMPACT_SLACK_BYTES {
        let tail = read_tail(path, TRANSCRIPT_MAX_BYTES)?;
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &tail)?;
        fs::rename(&tmp_path, path)?;
    }

    Ok(())
}

fn read_tail(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn strip_script_banners(bytes: &[u8]) -> Vec<u8> {
    let start = if bytes.starts_with(b"Script started on ") {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    } else {
        0
    };

    let end = find_last_subslice(&bytes[start..], b"\nScript done on ").map_or(bytes.len(), |index| start + index);

    bytes[start..end].to_vec()
}

fn find_last_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    haystack.windows(needle.len()).rposition(|window| window == needle)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_clean_prompt_boundary(bytes: &mut Vec<u8>) {
    if bytes.is_empty() {
        return;
    }

    if matches!(bytes.last(), Some(b'\n' | b'\r')) {
        return;
    }

    bytes.extend_from_slice(b"\r\n");
}

fn wrap_launch_command_with_support(
    support: &ScriptSupport,
    session_path: &Path,
    program: String,
    args: Vec<String>,
) -> (String, Vec<String>) {
    match support {
        ScriptSupport::Missing | ScriptSupport::Unsupported { .. } => (program, args),
        ScriptSupport::Supported {
            program: script_program,
            flavor: ScriptFlavor::Bsd,
        } => build_bsd_script_command(script_program.clone(), session_path, program, args),
        ScriptSupport::Supported {
            program: script_program,
            flavor: ScriptFlavor::UtilLinux,
        } => build_util_linux_script_command(script_program.clone(), session_path, program, args),
    }
}

fn script_support() -> &'static ScriptSupport {
    SCRIPT_SUPPORT.get_or_init(|| {
        let support = ScriptSupport::detect(std::env::var_os("PATH").as_deref(), current_script_flavor());
        match &support {
            ScriptSupport::Missing => tracing::warn!("transcript capture disabled: `script` was not found in PATH"),
            ScriptSupport::Unsupported { reason } => {
                tracing::warn!("transcript capture disabled: {reason}");
            }
            ScriptSupport::Supported { .. } => {}
        }
        support
    })
}

impl ScriptSupport {
    fn detect(path_env: Option<&OsStr>, flavor: ScriptFlavor) -> Self {
        let mut first_error = None;

        for candidate in script_program_candidates(path_env) {
            match probe_script_support(&candidate, flavor) {
                Ok(()) => {
                    return Self::Supported {
                        program: candidate.display().to_string(),
                        flavor,
                    };
                }
                Err(reason) => {
                    if first_error.is_none() {
                        first_error = Some(format!(
                            "`{}` rejected the Horizon transcript probe: {reason}",
                            candidate.display()
                        ));
                    }
                }
            }
        }

        first_error.map_or(Self::Missing, |reason| Self::Unsupported { reason })
    }
}

fn current_script_flavor() -> ScriptFlavor {
    if USES_BSD_SCRIPT {
        ScriptFlavor::Bsd
    } else {
        ScriptFlavor::UtilLinux
    }
}

fn script_program_candidates(path_env: Option<&OsStr>) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let _ = path_env;
        Vec::new()
    }

    #[cfg(not(windows))]
    {
        let Some(path) = path_env else {
            return Vec::new();
        };

        let mut candidates = Vec::new();
        for candidate in std::env::split_paths(path).map(|dir| dir.join("script")) {
            if candidate.is_file() && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        candidates
    }
}

#[cfg(not(windows))]
fn probe_script_support(script_program: &Path, flavor: ScriptFlavor) -> std::result::Result<(), String> {
    let session_path = probe_session_path();
    let (_, args) = match flavor {
        ScriptFlavor::Bsd => build_bsd_script_command(
            script_program.display().to_string(),
            &session_path,
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "exit 0".to_string()],
        ),
        ScriptFlavor::UtilLinux => build_util_linux_script_command(
            script_program.display().to_string(),
            &session_path,
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "exit 0".to_string()],
        ),
    };

    let mut busy_retries = 0u8;
    let output = loop {
        match Command::new(script_program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) => break output,
            Err(error) if error.raw_os_error() == Some(26) && busy_retries < 5 => {
                busy_retries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(format!("failed to launch probe: {error}")),
        }
    };

    let _ = fs::remove_file(&session_path);

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        format!("probe exited with {}", output.status)
    } else {
        stderr
    };

    Err(detail)
}

#[cfg(windows)]
fn probe_script_support(_script_program: &Path, _flavor: ScriptFlavor) -> std::result::Result<(), String> {
    Err("`script` is unavailable on Windows".to_string())
}

#[cfg(not(windows))]
fn probe_session_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "horizon-script-probe-{}-{timestamp}.session",
        std::process::id()
    ))
}

fn build_bsd_script_command(
    script_program: String,
    session_path: &Path,
    program: String,
    args: Vec<String>,
) -> (String, Vec<String>) {
    let mut script_args = vec![
        "-q".to_string(),
        "-e".to_string(),
        "-F".to_string(),
        session_path.display().to_string(),
        program,
    ];
    script_args.extend(args);
    (script_program, script_args)
}

fn build_util_linux_script_command(
    script_program: String,
    session_path: &Path,
    program: String,
    args: Vec<String>,
) -> (String, Vec<String>) {
    let mut parts = vec![program];
    parts.extend(args);
    let command = parts
        .iter()
        .map(|part| shell_escape(part))
        .collect::<Vec<_>>()
        .join(" ");

    (
        script_program,
        vec![
            "-qef".to_string(),
            "--log-out".to_string(),
            session_path.display().to_string(),
            "--command".to_string(),
            command,
        ],
    )
}

fn shell_escape(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(not(windows))]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    #[cfg(not(windows))]
    use std::path::PathBuf;

    use super::{
        PanelKind, PanelTranscript, ScriptFlavor, ScriptSupport, build_bsd_script_command,
        build_util_linux_script_command, ensure_clean_prompt_boundary, strip_script_banners,
        wrap_launch_command_with_support,
    };
    use uuid::Uuid;

    #[test]
    fn strip_script_banners_removes_headers_and_footers() {
        let bytes = concat!(
            "Script started on 2026-03-16 00:00:00+01:00 [COMMAND=\"bash\" <not executed on terminal>]\n",
            "\u{001b}[31mhello\u{001b}[0m\r\n\n",
            "Script done on 2026-03-16 00:00:01+01:00 [COMMAND_EXIT_CODE=\"0\"]\n",
        )
        .as_bytes()
        .to_vec();

        assert_eq!(strip_script_banners(&bytes), b"\x1b[31mhello\x1b[0m\r\n".to_vec());
    }

    #[test]
    fn strip_script_banners_keeps_unfinished_session_payload() {
        let bytes = concat!(
            "Script started on 2026-03-16 00:00:00+01:00 [COMMAND=\"bash\" <not executed on terminal>]\n",
            "partial output",
        )
        .as_bytes()
        .to_vec();

        assert_eq!(strip_script_banners(&bytes), b"partial output".to_vec());
    }

    #[test]
    fn transcript_is_only_created_for_non_agent_panels() {
        assert!(PanelTranscript::for_panel(PanelKind::Shell, Some("/tmp".into()), "panel-1").is_some());
        assert!(PanelTranscript::for_panel(PanelKind::Ssh, Some("/tmp".into()), "panel-1").is_some());
        assert!(PanelTranscript::for_panel(PanelKind::Command, Some("/tmp".into()), "panel-1").is_some());
        assert!(PanelTranscript::for_panel(PanelKind::Codex, Some("/tmp".into()), "panel-1").is_none());
        assert!(PanelTranscript::for_panel(PanelKind::Claude, Some("/tmp".into()), "panel-1").is_none());
        assert!(PanelTranscript::for_panel(PanelKind::OpenCode, Some("/tmp".into()), "panel-1").is_none());
    }

    #[test]
    fn prepare_replay_bytes_finalizes_live_session_log() {
        let root = std::env::temp_dir().join(format!("horizon-transcript-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp dir");
        let transcript =
            PanelTranscript::for_panel(PanelKind::Shell, Some(root.clone()), "panel-7").expect("shell transcript");
        let session_path = transcript.session_path();
        fs::write(
            &session_path,
            concat!(
                "Script started on 2026-03-16 00:00:00+01:00 [COMMAND=\"bash\" <not executed on terminal>]\n",
                "first line\r\n",
                "second line",
            ),
        )
        .expect("session log");

        let replay = transcript.prepare_replay_bytes().expect("replay bytes");

        assert_eq!(replay, b"first line\r\nsecond line\r\n".to_vec());
        assert!(!session_path.exists());
        assert_eq!(
            fs::read(transcript.history_path()).expect("history"),
            b"first line\r\nsecond line".to_vec()
        );

        transcript.delete_all().expect("cleanup");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_clean_prompt_boundary_appends_newline_to_partial_lines() {
        let mut bytes = b"nvtop footer".to_vec();

        ensure_clean_prompt_boundary(&mut bytes);

        assert_eq!(bytes, b"nvtop footer\r\n".to_vec());
    }

    #[test]
    fn build_script_command_uses_bsd_syntax() {
        let (program, args) = build_bsd_script_command(
            "/usr/bin/script".to_string(),
            Path::new("/tmp/panel.session"),
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "echo hi".to_string()],
        );

        assert_eq!(program, "/usr/bin/script");
        assert_eq!(
            args,
            vec![
                "-q".to_string(),
                "-e".to_string(),
                "-F".to_string(),
                "/tmp/panel.session".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ]
        );
    }

    #[test]
    fn build_script_command_uses_util_linux_syntax() {
        let (program, args) = build_util_linux_script_command(
            "/usr/bin/script".to_string(),
            Path::new("/tmp/panel.session"),
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "echo hi".to_string()],
        );

        assert_eq!(program, "/usr/bin/script");
        assert_eq!(
            args,
            vec![
                "-qef".to_string(),
                "--log-out".to_string(),
                "/tmp/panel.session".to_string(),
                "--command".to_string(),
                "/bin/sh -c 'echo hi'".to_string(),
            ]
        );
    }

    #[test]
    fn wrap_launch_command_skips_transcript_capture_when_script_is_unavailable() {
        let session_path = Path::new("/tmp/panel.session");
        let original_args = vec!["-c".to_string(), "echo hi".to_string()];
        let original = ("/bin/sh".to_string(), original_args.clone());

        let wrapped = wrap_launch_command_with_support(
            &ScriptSupport::Missing,
            session_path,
            original.0.clone(),
            original.1.clone(),
        );

        assert_eq!(wrapped, original);
    }

    #[test]
    fn detect_script_support_reports_missing_script() {
        let support = ScriptSupport::detect(None, ScriptFlavor::UtilLinux);

        assert_eq!(support, ScriptSupport::Missing);
    }

    #[test]
    fn wrap_launch_command_skips_transcript_capture_when_script_is_incompatible() {
        let session_path = Path::new("/tmp/panel.session");
        let original_args = vec!["-c".to_string(), "echo hi".to_string()];
        let original = ("/bin/sh".to_string(), original_args.clone());

        let wrapped = wrap_launch_command_with_support(
            &ScriptSupport::Unsupported {
                reason: "unsupported flags".to_string(),
            },
            session_path,
            original.0.clone(),
            original.1.clone(),
        );

        assert_eq!(wrapped, original);
    }

    #[cfg(not(windows))]
    #[test]
    fn detect_script_support_reports_incompatible_script() {
        let root = temp_script_root("incompatible");
        let script_path = root.join("script");
        write_executable_script(
            &script_path,
            r"#!/bin/sh
echo unsupported flags >&2
exit 64
",
        );

        let support = ScriptSupport::detect(Some(root.as_os_str()), ScriptFlavor::UtilLinux);

        match support {
            ScriptSupport::Unsupported { reason } => {
                assert!(reason.contains("unsupported flags"), "{reason}");
            }
            other => panic!("expected unsupported script support, got {other:?}"),
        }

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(windows))]
    #[test]
    fn detect_script_support_accepts_util_linux_probe_shape() {
        let root = temp_script_root("util-linux");
        let script_path = root.join("script");
        write_executable_script(
            &script_path,
            r#"#!/bin/sh
[ "$1" = "-qef" ] || exit 11
[ "$2" = "--log-out" ] || exit 12
[ "$4" = "--command" ] || exit 13
[ "$5" = "/bin/sh -c 'exit 0'" ] || exit 14
: > "$3"
exit 0
"#,
        );

        let support = ScriptSupport::detect(Some(root.as_os_str()), ScriptFlavor::UtilLinux);

        assert_eq!(
            support,
            ScriptSupport::Supported {
                program: script_path.display().to_string(),
                flavor: ScriptFlavor::UtilLinux,
            }
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(windows))]
    #[test]
    fn detect_script_support_accepts_bsd_probe_shape() {
        let root = temp_script_root("bsd");
        let script_path = root.join("script");
        write_executable_script(
            &script_path,
            r#"#!/bin/sh
[ "$1" = "-q" ] || exit 21
[ "$2" = "-e" ] || exit 22
[ "$3" = "-F" ] || exit 23
[ "$5" = "/bin/sh" ] || exit 24
[ "$6" = "-c" ] || exit 25
[ "$7" = "exit 0" ] || exit 26
: > "$4"
exit 0
"#,
        );

        let support = ScriptSupport::detect(Some(root.as_os_str()), ScriptFlavor::Bsd);

        assert_eq!(
            support,
            ScriptSupport::Supported {
                program: script_path.display().to_string(),
                flavor: ScriptFlavor::Bsd,
            }
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(not(windows))]
    fn temp_script_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("horizon-transcript-script-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp script root");
        root
    }

    #[cfg(not(windows))]
    fn write_executable_script(path: &Path, contents: &str) {
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, contents).expect("script body");
        let mut permissions = fs::metadata(&temp_path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&temp_path, permissions).expect("script permissions");
        fs::rename(&temp_path, path).expect("script rename");
    }
}
