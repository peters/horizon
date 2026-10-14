//! Reads the result and the evidence that the agent on a test cloud wrote, over the test
//! cloud's SSH connection. Every byte comes from that agent, and code of the pull request ran
//! beside it: the result is parsed into a fixed shape, names and sizes are checked again
//! here, and only files of known kinds are kept.
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

use super::{REPORT_TEXT_LIMIT, State};
use crate::cloud_runtime::{Error, Result, command::Runner, ssh::Connection};

const PROBE: &str = include_str!("probe.py");
const TIMEOUT: Duration = Duration::from_secs(60);
/// The most evidence files kept from one test besides [`TEXTS`], and their total size.
const FILES: usize = 12;
/// The result and the report, which Horizon keeps besides the evidence files.
const TEXTS: [&str; 2] = ["result.json", "report.md"];
const FILE_LIMIT: u64 = 2 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 16 * 1024 * 1024;
const SUMMARY_LIMIT: usize = 300;
/// The kinds of evidence Horizon keeps: pictures, recordings and text.
const KINDS: [&str; 10] = ["png", "jpg", "jpeg", "gif", "webp", "mp4", "webm", "md", "txt", "json"];

/// What the agent reported, once it wrote `result.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// [`State::Passed`], [`State::Failed`] or [`State::Blocked`].
    pub state: State,
    pub summary: String,
}

/// What the test cloud holds now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` until the agent wrote a valid `result.json`.
    pub outcome: Option<Outcome>,
    pub report: String,
    /// The evidence files that Horizon would keep, with their sizes.
    pub files: Vec<(String, u64)>,
}

#[derive(Deserialize)]
struct RawStatus {
    result: Option<String>,
    report: Option<String>,
    #[serde(default)]
    files: Vec<RawFile>,
}

#[derive(Deserialize)]
struct RawFile {
    name: String,
    size: u64,
}

fn command(arguments: &str) -> String {
    let script = base64::engine::general_purpose::STANDARD.encode(PROBE);
    format!(
        "launcher=python3; if [ -f /run/horizon-tailnet/agent-isolation ]; then launcher='horizon-worker-tailnet agent python3'; fi; \
printf %s '{script}' | base64 -d | $launcher -I - {arguments}"
    )
}

/// Reads the result and the list of evidence files of the test cloud behind `connection`.
/// # Errors
/// A failed connection, or an answer that is not a status.
pub fn status(connection: &Connection, runner: &Runner<'_>) -> Result<Status> {
    let output = runner.run_parsed(
        "Native test status",
        &mut connection.pinned_command(&command("status")),
        TIMEOUT,
    )?;
    parse_status(&output)
}

/// Parses what the probe printed for a status.
/// # Errors
/// Output that is not one status object.
pub fn parse_status(output: &str) -> Result<Status> {
    let raw: RawStatus =
        serde_json::from_str(output.trim()).map_err(|_| Error::Invalid("The test cloud's answer is malformed"))?;
    let outcome = raw.result.as_deref().and_then(outcome);
    let mut files: Vec<(String, u64)> = Vec::new();
    for file in raw.files {
        // The result and the report do not take the place of evidence.
        let text = TEXTS.contains(&file.name.as_str());
        let evidence = files.iter().filter(|(name, _)| !TEXTS.contains(&name.as_str())).count();
        // Windows and macOS folders ignore case, so a second spelling would overwrite the first.
        let listed = files.iter().any(|(name, _)| name.eq_ignore_ascii_case(&file.name));
        if (text || evidence < FILES) && !listed && kept(&file.name) && file.size <= FILE_LIMIT {
            files.push((file.name, file.size));
        }
    }
    Ok(Status {
        outcome,
        report: raw
            .report
            .map(|text| clean(&text, REPORT_TEXT_LIMIT, true))
            .unwrap_or_default(),
        files,
    })
}

/// The outcome in the text of `result.json`, or `None` when it is not one.
fn outcome(text: &str) -> Option<Outcome> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let state = match value.get("outcome")?.as_str()? {
        "passed" => State::Passed,
        "failed" => State::Failed,
        "blocked" => State::Blocked,
        _ => return None,
    };
    let summary = value
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Some(Outcome {
        state,
        summary: clean(summary, SUMMARY_LIMIT, false),
    })
}

/// `text` without control characters other than line breaks and tabs when `lines` is set,
/// cut to at most `limit` bytes at a character boundary.
fn clean(text: &str, limit: usize, lines: bool) -> String {
    let mut kept = String::new();
    for c in text.chars() {
        let c = match c {
            '\n' | '\t' if lines => c,
            '\n' | '\t' | '\r' => ' ',
            c if c.is_control() => continue,
            c => c,
        };
        if kept.len() + c.len_utf8() > limit {
            break;
        }
        kept.push(c);
    }
    kept
}

/// Whether Horizon keeps an evidence file named `name`: a plain name of a known kind that
/// Windows does not take for a device.
fn kept(name: &str) -> bool {
    let plain = (1..=80).contains(&name.len())
        && !name.starts_with('.')
        && !crate::cloud_runtime::repository::source::windows_device(name)
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    plain
        && name
            .rsplit_once('.')
            .is_some_and(|(_, kind)| KINDS.contains(&kind.to_ascii_lowercase().as_str()))
}

/// Copies the evidence that `status` lists into the new folder `destination`, within the
/// limits on count and total size. Returns the names of the files it kept. A file that cannot
/// be read is left out.
/// # Errors
/// The folder cannot be made, or a file cannot be written.
pub fn collect(
    connection: &Connection,
    runner: &Runner<'_>,
    status: &Status,
    destination: &Path,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(destination)?;
    let mut kept_files = Vec::new();
    let mut total = 0_u64;
    for (name, size) in &status.files {
        if total + size > TOTAL_LIMIT {
            continue;
        }
        let Ok(output) = runner.run_parsed(
            "Native test evidence",
            &mut connection.pinned_command(&command(&format!("file {name}"))),
            TIMEOUT,
        ) else {
            continue;
        };
        let Ok(data) = base64::engine::general_purpose::STANDARD.decode(output.trim()) else {
            continue;
        };
        // The file may have grown since the status listed it: the bytes read count.
        if !admit(&mut total, data.len() as u64) {
            continue;
        }
        std::fs::write(file(destination, name), &data)?;
        kept_files.push(name.clone());
    }
    Ok(kept_files)
}

/// Whether a file of `size` bytes fits the limits after `total` bytes kept; adds it when so.
fn admit(total: &mut u64, size: u64) -> bool {
    if size > FILE_LIMIT || total.saturating_add(size) > TOTAL_LIMIT {
        return false;
    }
    *total += size;
    true
}

fn file(destination: &Path, name: &str) -> PathBuf {
    destination.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_known_outcome_counts_and_text_loses_its_control_characters() {
        let status = parse_status(
            r#"{"result": "{\"outcome\": \"passed\", \"summary\": \"all\\u001b[31m green\"}", "report": "| a |\n| b |\u0007", "files": []}"#,
        )
        .unwrap();
        assert_eq!(
            status.outcome,
            Some(Outcome {
                state: State::Passed,
                summary: "all[31m green".into()
            })
        );
        assert_eq!(status.report, "| a |\n| b |");
        for result in [r#""{\"outcome\": \"merged\"}""#, r#""not json""#, "null"] {
            let status = parse_status(&format!(r#"{{"result": {result}, "report": null}}"#)).unwrap();
            assert_eq!(status.outcome, None, "{result}");
        }
        assert!(parse_status("{]").is_err());
    }

    #[test]
    fn only_plain_names_of_known_kinds_within_the_limits_are_kept() {
        let files: Vec<_> = (0..20)
            .map(|n| serde_json::json!({"name": format!("shot-{n}.png"), "size": 10}))
            .chain([
                serde_json::json!({"name": "../etc/passwd.txt", "size": 1}),
                serde_json::json!({"name": ".hidden.png", "size": 1}),
                serde_json::json!({"name": "tool.sh", "size": 1}),
                serde_json::json!({"name": "big.mp4", "size": FILE_LIMIT + 1}),
            ])
            .collect();
        let status =
            parse_status(&serde_json::json!({"result": null, "report": null, "files": files}).to_string()).unwrap();
        assert_eq!(status.files.len(), FILES);
        assert!(status.files.iter().all(|(name, _)| name.starts_with("shot-")));
        assert!(kept("Report.MD"));
        assert!(!kept("con.png") && !kept("LPT9.md") && kept("console.png"));
        let names = ["shot.png", "SHOT.png", "nul.txt", "com1.png", "console.png"]
            .map(|name| serde_json::json!({"name": name, "size": 1}));
        let status =
            parse_status(&serde_json::json!({"result": null, "report": null, "files": names}).to_string()).unwrap();
        assert_eq!(
            status.files,
            [("shot.png".to_owned(), 1), ("console.png".to_owned(), 1)],
            "a second spelling and a device name are left out"
        );
        let with_texts: Vec<_> = files
            .iter()
            .cloned()
            .chain(TEXTS.map(|name| serde_json::json!({"name": name, "size": 10})))
            .collect();
        let status =
            parse_status(&serde_json::json!({"result": null, "report": null, "files": with_texts}).to_string())
                .unwrap();
        assert_eq!(
            status.files.len(),
            FILES + TEXTS.len(),
            "the result and the report come on top"
        );
    }

    #[test]
    fn the_bytes_read_count_against_the_limits() {
        let mut total = TOTAL_LIMIT - FILE_LIMIT;
        assert!(admit(&mut total, FILE_LIMIT));
        assert!(!admit(&mut total, 1), "a file listed as empty that grew does not fit");
        assert_eq!(total, TOTAL_LIMIT);
        assert!(!admit(&mut 0, FILE_LIMIT + 1));
    }

    #[test]
    fn the_report_is_cut_at_a_character_boundary() {
        let text = "æ".repeat(REPORT_TEXT_LIMIT);
        let cut = clean(&text, REPORT_TEXT_LIMIT, true);
        assert!(cut.len() <= REPORT_TEXT_LIMIT);
        assert!(cut.chars().all(|c| c == 'æ'));
    }
}
