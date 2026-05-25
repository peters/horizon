use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// Lookback window for which jsonl files are considered "active enough" to
/// scan. Files older than this are skipped to keep steady-state polling cheap.
const LOOKBACK_SECS: u64 = 14 * 86_400;

/// Live loader that scans `~/.claude/projects/*.jsonl` and aggregates
/// per-day token totals from assistant messages.
///
/// Parse results are cached per-file by `(mtime, size)` so unchanged files
/// are not re-read between polls.
pub(super) struct ClaudeLiveLoader {
    cache: HashMap<PathBuf, CachedFile>,
}

#[derive(Default)]
pub(super) struct ClaudeLiveStats {
    pub by_day: HashMap<String, ClaudeDayTotals>,
}

#[derive(Default, Clone, Copy)]
pub(super) struct ClaudeDayTotals {
    pub tokens: u64,
    pub messages: u32,
}

struct CachedFile {
    mtime_secs: u64,
    size: u64,
    per_day: HashMap<String, ClaudeDayTotals>,
}

#[derive(Deserialize)]
struct AssistantLine {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    timestamp: String,
    #[serde(default)]
    message: AssistantMessage,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AssistantMessage {
    usage: AssistantUsage,
}

#[derive(Default, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_field_names)] // field names mirror the JSON schema exactly
struct AssistantUsage {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens: u64,
}

impl ClaudeLiveLoader {
    pub(super) fn new() -> Self {
        Self { cache: HashMap::new() }
    }

    /// Walk `~/.claude/projects/`, aggregate per-day token totals from jsonl
    /// files modified in the last 14 days. Files unchanged since the last
    /// call are served from the `(mtime, size)`-keyed cache.
    pub(super) fn collect(&mut self, _recent_dates: &[String]) -> ClaudeLiveStats {
        let Some(dir) = super::home_dir().map(|h| h.join(".claude/projects")) else {
            return ClaudeLiveStats::default();
        };
        if !dir.exists() {
            return ClaudeLiveStats::default();
        }
        self.collect_from_dir(&dir)
    }

    pub(super) fn collect_from_dir(&mut self, dir: &Path) -> ClaudeLiveStats {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let cutoff_secs = now_secs.saturating_sub(LOOKBACK_SECS);

        let files = walk_claude_projects(dir, cutoff_secs);
        let mut seen: HashSet<PathBuf> = HashSet::with_capacity(files.len());

        for (path, mtime_secs, size) in files {
            seen.insert(path.clone());
            let needs_reparse = self
                .cache
                .get(&path)
                .is_none_or(|cached| cached.mtime_secs != mtime_secs || cached.size != size);
            if needs_reparse {
                let per_day = parse_file(&path);
                self.cache.insert(
                    path,
                    CachedFile {
                        mtime_secs,
                        size,
                        per_day,
                    },
                );
            }
        }

        self.cache.retain(|p, _| seen.contains(p));

        let mut by_day: HashMap<String, ClaudeDayTotals> = HashMap::new();
        for cached in self.cache.values() {
            for (date, totals) in &cached.per_day {
                let entry = by_day.entry(date.clone()).or_default();
                entry.tokens = entry.tokens.saturating_add(totals.tokens);
                entry.messages = entry.messages.saturating_add(totals.messages);
            }
        }

        ClaudeLiveStats { by_day }
    }

    #[cfg(test)]
    pub(super) fn cached_files(&self) -> usize {
        self.cache.len()
    }
}

/// Recursive walk over `dir` returning `(path, mtime_secs, size)` for every
/// `.jsonl` file modified at or after `cutoff_secs`. Recurses into all
/// subdirectories, including `subagents/` (those turns still consumed tokens).
fn walk_claude_projects(dir: &Path, cutoff_secs: u64) -> Vec<(PathBuf, u64, u64)> {
    let mut out = Vec::new();
    walk_into(dir, cutoff_secs, &mut out);
    out
}

fn walk_into(dir: &Path, cutoff_secs: u64, out: &mut Vec<(PathBuf, u64, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk_into(&path, cutoff_secs, out);
            continue;
        }
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let mtime_secs = modified.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if mtime_secs < cutoff_secs {
            continue;
        }
        out.push((path, mtime_secs, meta.len()));
    }
}

/// Parse a single jsonl file and aggregate assistant-message token usage
/// per UTC day (taken from the first 10 chars of `timestamp`).
///
/// Lines that are not assistant messages are skipped via a cheap substring
/// pre-check. Malformed JSON lines and assistant lines without `usage` still
/// have well-defined behavior: the former are skipped, the latter contribute
/// 0 tokens but increment the day's message counter (they are real assistant
/// turns; the usage object just happens to be absent or unparseable).
fn parse_file(path: &Path) -> HashMap<String, ClaudeDayTotals> {
    let mut per_day: HashMap<String, ClaudeDayTotals> = HashMap::new();
    let Ok(file) = File::open(path) else {
        return per_day;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        if !line_has_assistant_type(&line) {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<AssistantLine>(&line) else {
            continue;
        };
        if parsed.kind != "assistant" {
            continue;
        }
        if parsed.timestamp.len() < 10 {
            continue;
        }
        let day = parsed.timestamp[..10].to_string();
        let usage = &parsed.message.usage;
        let tokens = usage
            .input_tokens
            .saturating_add(usage.cache_creation_input_tokens)
            .saturating_add(usage.cache_read_input_tokens)
            .saturating_add(usage.output_tokens);
        let entry = per_day.entry(day).or_default();
        entry.tokens = entry.tokens.saturating_add(tokens);
        entry.messages = entry.messages.saturating_add(1);
    }
    per_day
}

fn line_has_assistant_type(line: &str) -> bool {
    line.match_indices("\"type\"").any(|(start, _)| {
        let rest = &line[start + "\"type\"".len()..];
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            return false;
        };
        rest.trim_start().starts_with("\"assistant\"")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut file = fs::File::create(path).expect("create jsonl");
        for line in lines {
            writeln!(file, "{line}").expect("write line");
        }
    }

    #[test]
    fn parse_file_aggregates_assistant_usage_by_timestamp_date() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("session.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"user","timestamp":"2026-05-24T10:00:00.000Z","message":{}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-24T10:05:00.000Z","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_read_input_tokens":200,"output_tokens":30}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-24T11:00:00.000Z","message":{"usage":{"input_tokens":5,"cache_creation_input_tokens":50,"cache_read_input_tokens":150,"output_tokens":20}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-25T08:00:00.000Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-24T12:00:00.000Z","message":{}}"#,
                r"this line is not json",
            ],
        );

        let per_day = parse_file(&path);

        // 2026-05-24: two assistant lines with usage (10+100+200+30=340 and 5+50+150+20=225 = 565)
        // plus one assistant line without usage (0 tokens but +1 message).
        let day_24 = per_day.get("2026-05-24").copied().expect("2026-05-24 bucket");
        assert_eq!(day_24.tokens, 565);
        assert_eq!(day_24.messages, 3);

        // 2026-05-25: one assistant line (1+2+3+4=10 tokens, 1 message).
        let day_25 = per_day.get("2026-05-25").copied().expect("2026-05-25 bucket");
        assert_eq!(day_25.tokens, 10);
        assert_eq!(day_25.messages, 1);

        // Malformed line and user line are ignored.
        assert_eq!(per_day.len(), 2);
    }

    #[test]
    fn claude_live_loader_walks_subagents_and_recurses() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("proj-a/subagents")).expect("mkdir subagents");
        fs::create_dir_all(dir.path().join("proj-b")).expect("mkdir proj-b");

        let line = r#"{"type":"assistant","timestamp":"2026-05-24T10:00:00.000Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}"#;

        write_lines(&dir.path().join("proj-a/foo.jsonl"), &[line]);
        write_lines(&dir.path().join("proj-a/subagents/bar.jsonl"), &[line]);
        write_lines(&dir.path().join("proj-b/baz.jsonl"), &[line]);

        let mut loader = ClaudeLiveLoader::new();
        let stats = loader.collect_from_dir(dir.path());

        // Three files of 10 tokens each = 30 tokens, 3 messages on 2026-05-24.
        let day = stats.by_day.get("2026-05-24").copied().expect("2026-05-24 totals");
        assert_eq!(day.tokens, 30);
        assert_eq!(day.messages, 3);
    }

    #[test]
    fn parse_file_accepts_whitespace_around_type_separator() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("session.jsonl");
        write_lines(
            &path,
            &[
                r#"{ "type" : "assistant", "timestamp":"2026-05-24T10:00:00.000Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}"#,
            ],
        );

        let per_day = parse_file(&path);

        let day = per_day.get("2026-05-24").copied().expect("2026-05-24 bucket");
        assert_eq!(day.tokens, 10);
        assert_eq!(day.messages, 1);
    }

    #[test]
    fn claude_live_loader_reuses_cache_for_unchanged_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("session.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"assistant","timestamp":"2026-05-24T10:00:00.000Z","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}"#,
            ],
        );

        let mut loader = ClaudeLiveLoader::new();
        let _ = loader.collect_from_dir(dir.path());
        assert_eq!(loader.cached_files(), 1);

        let _ = loader.collect_from_dir(dir.path());
        assert_eq!(loader.cached_files(), 1);

        fs::remove_file(&path).expect("rm jsonl");
        let _ = loader.collect_from_dir(dir.path());
        assert_eq!(loader.cached_files(), 0);
    }

    #[test]
    fn assistant_line_deserialization_tolerates_unknown_fields() {
        let line = r#"{
            "parentUuid": "e6139ceb-5fd7-4d04-b377-0759b4357e0a",
            "isSidechain": false,
            "message": {
                "model": "claude-opus-4-7",
                "id": "msg_01KnicQUNSXzireRha82Xqe7",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "x"}],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 6,
                    "cache_creation_input_tokens": 7617,
                    "cache_read_input_tokens": 17317,
                    "output_tokens": 326,
                    "service_tier": "standard",
                    "iterations": [{"input_tokens": 6, "output_tokens": 326}]
                },
                "diagnostics": null
            },
            "requestId": "req_011CbNnbnkqHUbbF3G2hZK9q",
            "type": "assistant",
            "uuid": "b20a798c-487a-4efc-8ad1-c0af13c5556b",
            "timestamp": "2026-05-25T06:08:05.576Z",
            "userType": "external",
            "entrypoint": "cli",
            "cwd": "/home/peters/github/nativesdk",
            "sessionId": "ad040111-fc9b-4497-9787-c6af1f41eeb2",
            "version": "2.1.150",
            "gitBranch": "main"
        }"#;

        let parsed: AssistantLine = serde_json::from_str(line).expect("decode realistic line");
        assert_eq!(parsed.kind, "assistant");
        assert_eq!(&parsed.timestamp[..10], "2026-05-25");
        let usage = &parsed.message.usage;
        assert_eq!(usage.input_tokens, 6);
        assert_eq!(usage.cache_creation_input_tokens, 7617);
        assert_eq!(usage.cache_read_input_tokens, 17317);
        assert_eq!(usage.output_tokens, 326);
    }
}
