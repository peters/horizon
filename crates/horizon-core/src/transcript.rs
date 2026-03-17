use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::panel::PanelKind;

const TRANSCRIPT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const TRANSCRIPT_COMPACT_SLACK_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelTranscript {
    root: PathBuf,
    local_id: String,
}

impl PanelTranscript {
    #[must_use]
    pub fn for_panel(kind: PanelKind, root: Option<PathBuf>, local_id: &str) -> Option<Self> {
        if !matches!(kind, PanelKind::Shell | PanelKind::Command) {
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
    pub fn wrap_launch_command(&self, program: String, args: Vec<String>) -> (String, Vec<String>) {
        let Some(script_program) = script_program() else {
            return (program, args);
        };

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
                self.session_path().display().to_string(),
                "--command".to_string(),
                command,
            ],
        )
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

fn script_program() -> Option<String> {
    #[cfg(windows)]
    {
        None
    }

    #[cfg(not(windows))]
    {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("script"))
            .find(|candidate| candidate.is_file())
            .map(|candidate| candidate.display().to_string())
    }
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

    use super::{PanelKind, PanelTranscript, ensure_clean_prompt_boundary, strip_script_banners};
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
        assert!(PanelTranscript::for_panel(PanelKind::Command, Some("/tmp".into()), "panel-1").is_some());
        assert!(PanelTranscript::for_panel(PanelKind::Codex, Some("/tmp".into()), "panel-1").is_none());
        assert!(PanelTranscript::for_panel(PanelKind::Claude, Some("/tmp".into()), "panel-1").is_none());
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
}
