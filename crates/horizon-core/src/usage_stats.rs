use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use serde::Deserialize;

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Snapshot of usage statistics for Claude Code and Codex CLI.
pub struct UsageSnapshot {
    pub claude: ToolUsage,
    pub codex: ToolUsage,
    pub daily: Vec<DailyUsage>,
    pub updated_at: Instant,
}

/// Aggregate statistics for a single tool (today + this week).
#[derive(Default)]
pub struct ToolUsage {
    pub today_sessions: u32,
    pub today_tokens: u64,
    pub today_messages: u32,
    pub week_sessions: u32,
    pub week_tokens: u64,
    pub week_messages: u32,
}

/// Per-day breakdown with both tools.
pub struct DailyUsage {
    pub date: String,
    pub claude_sessions: u32,
    pub claude_tokens: u64,
    pub codex_sessions: u32,
    pub codex_tokens: u64,
}

/// Spawn a background polling thread that reads usage data every 30 seconds.
///
/// Returns a receiver that yields `UsageSnapshot` values.
pub fn spawn_usage_poll() -> mpsc::Receiver<UsageSnapshot> {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        loop {
            let snapshot = collect_snapshot();
            if tx.send(snapshot).is_err() {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    });

    rx
}

/// Format a token count for display: "0", "45K", "1.2M", "3.4B".
#[must_use]
pub fn format_tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format_scaled_tokens(n, 1_000, 'K');
    }
    if n < 1_000_000_000 {
        return format_scaled_tokens(n, 1_000_000, 'M');
    }
    format_scaled_tokens(n, 1_000_000_000, 'B')
}

fn format_scaled_tokens(n: u64, divisor: u64, suffix: char) -> String {
    let whole = n / divisor;
    let remainder = n % divisor;

    if whole < 10 {
        let mut rounded_whole = whole;
        let mut tenths = (remainder.saturating_mul(10) + divisor / 2) / divisor;
        if tenths == 10 {
            rounded_whole = rounded_whole.saturating_add(1);
            tenths = 0;
        }
        format!("{rounded_whole}.{tenths}{suffix}")
    } else {
        let rounded_whole = whole.saturating_add(u64::from(remainder >= divisor / 2));
        format!("{rounded_whole}{suffix}")
    }
}

fn collect_snapshot() -> UsageSnapshot {
    let today = today_date_string();
    let week_dates = last_n_days_dates(7);
    let fortnight_dates = last_n_days_dates(14);

    let claude_data = load_claude_stats();
    let codex_data = load_codex_stats();

    let claude_today_extra_sessions = count_claude_today_sessions();

    let claude = build_tool_usage(&claude_data, &today, &week_dates, claude_today_extra_sessions);
    let codex = build_tool_usage_codex(&codex_data, &today, &week_dates);
    let daily = build_daily(&claude_data, &codex_data, &fortnight_dates);

    UsageSnapshot {
        claude,
        codex,
        daily,
        updated_at: Instant::now(),
    }
}

/// Build `ToolUsage` for Claude Code from the stats cache.
fn build_tool_usage(
    data: &ClaudeStatsCache,
    today: &str,
    week_dates: &[String],
    extra_today_sessions: u32,
) -> ToolUsage {
    let mut usage = ToolUsage::default();

    for day in &data.daily_activity {
        if day.date == today {
            usage.today_sessions = day.session_count.saturating_add(extra_today_sessions);
            usage.today_messages = day.message_count;
        }
        if week_dates.iter().any(|d| d == &day.date) {
            usage.week_sessions = usage.week_sessions.saturating_add(day.session_count);
            usage.week_messages = usage.week_messages.saturating_add(day.message_count);
        }
    }

    // If daily_activity had no entry for today but we found live sessions, use that.
    if usage.today_sessions == 0 && extra_today_sessions > 0 {
        usage.today_sessions = extra_today_sessions;
    }
    // Ensure week includes today
    if usage.week_sessions == 0 && usage.today_sessions > 0 {
        usage.week_sessions = usage.today_sessions;
        usage.week_messages = usage.today_messages;
    }

    for day in &data.daily_model_tokens {
        let day_total: u64 = day.tokens_by_model.values().sum();
        if day.date == today {
            usage.today_tokens = day_total;
        }
        if week_dates.iter().any(|d| d == &day.date) {
            usage.week_tokens = usage.week_tokens.saturating_add(day_total);
        }
    }

    usage
}

fn build_tool_usage_codex(data: &[CodexDayRow], today: &str, week_dates: &[String]) -> ToolUsage {
    let mut usage = ToolUsage::default();

    for row in data {
        if row.day == today {
            usage.today_sessions = row.sessions;
            usage.today_tokens = row.total_tokens;
        }
        if week_dates.iter().any(|d| d == &row.day) {
            usage.week_sessions = usage.week_sessions.saturating_add(row.sessions);
            usage.week_tokens = usage.week_tokens.saturating_add(row.total_tokens);
        }
    }

    usage
}

fn build_daily(claude_data: &ClaudeStatsCache, codex_data: &[CodexDayRow], dates: &[String]) -> Vec<DailyUsage> {
    dates
        .iter()
        .map(|date| {
            let claude_sessions = claude_data
                .daily_activity
                .iter()
                .find(|d| d.date == *date)
                .map_or(0, |d| d.session_count);
            let claude_tokens: u64 = claude_data
                .daily_model_tokens
                .iter()
                .find(|d| d.date == *date)
                .map_or(0, |d| d.tokens_by_model.values().sum());
            let codex_row = codex_data.iter().find(|r| r.day == *date);
            let codex_sessions = codex_row.map_or(0, |r| r.sessions);
            let codex_tokens = codex_row.map_or(0, |r| r.total_tokens);

            DailyUsage {
                date: date.clone(),
                claude_sessions,
                claude_tokens,
                codex_sessions,
                codex_tokens,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Claude Code stats-cache.json
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ClaudeStatsCache {
    daily_activity: Vec<ClaudeDailyActivity>,
    daily_model_tokens: Vec<ClaudeDailyModelTokens>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeDailyActivity {
    date: String,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    session_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeDailyModelTokens {
    date: String,
    #[serde(default)]
    tokens_by_model: std::collections::HashMap<String, u64>,
}

fn load_claude_stats() -> ClaudeStatsCache {
    let Some(path) = home_dir().map(|h| h.join(".claude/stats-cache.json")) else {
        return ClaudeStatsCache::default();
    };
    if !path.exists() {
        return ClaudeStatsCache::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => ClaudeStatsCache::default(),
    }
}

/// Count `.jsonl` files under `~/.claude/projects/` modified today as
/// an approximation of today's Claude session count (the stats cache lags).
fn count_claude_today_sessions() -> u32 {
    let Some(projects_dir) = home_dir().map(|h| h.join(".claude/projects")) else {
        return 0;
    };
    if !projects_dir.exists() {
        return 0;
    }
    let today = today_date_string();
    count_jsonl_modified_on(&projects_dir, &today)
}

fn count_jsonl_modified_on(dir: &std::path::Path, date_str: &str) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if path.file_name().and_then(std::ffi::OsStr::to_str) != Some("subagents") {
                count = count.saturating_add(count_jsonl_modified_on(&path, date_str));
            }
        } else if path.extension().and_then(std::ffi::OsStr::to_str) == Some("jsonl")
            && let Ok(meta) = std::fs::metadata(&path)
            && let Ok(modified) = meta.modified()
        {
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if unix_secs_to_date_string(secs) == date_str {
                count = count.saturating_add(1);
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Codex CLI state SQLite
// ---------------------------------------------------------------------------

struct CodexDayRow {
    day: String,
    sessions: u32,
    total_tokens: u64,
}

fn load_codex_stats() -> Vec<CodexDayRow> {
    let Some(path) = home_dir().map(|h| h.join(".codex/state_5.sqlite")) else {
        return Vec::new();
    };
    if !path.exists() {
        return Vec::new();
    }
    load_codex_stats_from_path(&path).unwrap_or_default()
}

fn load_codex_stats_from_path(path: &std::path::Path) -> Option<Vec<CodexDayRow>> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(path, flags).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT date(created_at, 'unixepoch') AS day, \
                    count(*) AS sessions, \
                    coalesce(sum(tokens_used), 0) AS total_tokens \
             FROM threads \
             GROUP BY day \
             ORDER BY day DESC \
             LIMIT 30",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CodexDayRow {
                day: row.get(0)?,
                sessions: row.get(1)?,
                total_tokens: row.get::<_, i64>(2).map(|v| v.max(0).cast_unsigned())?,
            })
        })
        .ok()?;

    let mut result = Vec::new();
    for row in rows.flatten() {
        result.push(row);
    }
    Some(result)
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

fn today_date_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_secs_to_date_string(secs)
}

fn unix_secs_to_date_string(secs: u64) -> String {
    // Simple date calculation avoiding chrono dependency.
    let days = secs / 86400;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn last_n_days_dates(n: u32) -> Vec<String> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (0..n)
        .map(|i| {
            let secs = now_secs.saturating_sub(u64::from(i) * 86400);
            unix_secs_to_date_string(secs)
        })
        .collect()
}

/// Convert a day count from Unix epoch to (year, month, day).
///
/// Algorithm from <https://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(days: u64) -> (i64, u64, u64) {
    let z = days.cast_signed() + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097).cast_unsigned();
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe.cast_signed() + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(4_500), "4.5K");
        assert_eq!(format_tokens(45_000), "45K");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_200_000), "1.2M");
        assert_eq!(format_tokens(320_000_000), "320M");
    }

    #[test]
    fn format_tokens_billions() {
        assert_eq!(format_tokens(3_400_000_000), "3.4B");
        assert_eq!(format_tokens(10_000_000_000), "10B");
    }

    #[test]
    fn civil_from_days_epoch() {
        let (y, m, d) = civil_from_days(0);
        assert_eq!((y, m, d), (1970_i64, 1_u64, 1_u64));
    }

    #[test]
    fn civil_from_days_known_date() {
        // 2026-03-16 is day 20528 from epoch
        let (y, m, d) = civil_from_days(20_528);
        assert_eq!((y, m, d), (2026_i64, 3_u64, 16_u64));
    }
}
