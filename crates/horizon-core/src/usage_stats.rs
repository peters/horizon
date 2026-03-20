use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use serde::Deserialize;

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Snapshot of usage statistics for Claude Code, Codex CLI, and `OpenCode`.
pub struct UsageSnapshot {
    pub claude: ToolUsage,
    pub codex: ToolUsage,
    pub opencode: ToolUsage,
    pub daily: Vec<DailyUsage>,
    pub updated_at: Instant,
}

/// Aggregate statistics for a single tool (today + this week).
#[derive(Default)]
pub struct ToolUsage {
    pub today_sessions: u32,
    pub today_tokens: u64,
    pub today_messages: u32,
    pub today_cost: f64,
    pub week_sessions: u32,
    pub week_tokens: u64,
    pub week_messages: u32,
    pub week_cost: f64,
}

/// Per-day breakdown with all tracked tools.
pub struct DailyUsage {
    pub date: String,
    pub claude_sessions: u32,
    pub claude_tokens: u64,
    pub codex_sessions: u32,
    pub codex_tokens: u64,
    pub opencode_sessions: u32,
    pub opencode_tokens: u64,
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

#[must_use]
pub fn format_cost(cost: f64) -> String {
    format!("${cost:.2}")
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
    let opencode_data = load_opencode_stats();

    let claude_today_extra_sessions = count_claude_today_sessions();

    let claude = build_tool_usage(&claude_data, &today, &week_dates, claude_today_extra_sessions);
    let codex = build_tool_usage_codex(&codex_data, &today, &week_dates);
    let opencode = build_tool_usage_opencode(&opencode_data, &today, &week_dates);
    let daily = build_daily(&claude_data, &codex_data, &opencode_data, &fortnight_dates);

    UsageSnapshot {
        claude,
        codex,
        opencode,
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

fn build_tool_usage_opencode(data: &[OpenCodeDayRow], today: &str, week_dates: &[String]) -> ToolUsage {
    let mut usage = ToolUsage::default();

    for row in data {
        if row.day == today {
            usage.today_sessions = row.sessions;
            usage.today_messages = row.messages;
            usage.today_tokens = row.total_tokens;
            usage.today_cost = row.total_cost;
        }
        if week_dates.iter().any(|d| d == &row.day) {
            usage.week_sessions = usage.week_sessions.saturating_add(row.sessions);
            usage.week_messages = usage.week_messages.saturating_add(row.messages);
            usage.week_tokens = usage.week_tokens.saturating_add(row.total_tokens);
            usage.week_cost += row.total_cost;
        }
    }

    usage
}

fn build_daily(
    claude_data: &ClaudeStatsCache,
    codex_data: &[CodexDayRow],
    opencode_data: &[OpenCodeDayRow],
    dates: &[String],
) -> Vec<DailyUsage> {
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
            let opencode_row = opencode_data.iter().find(|r| r.day == *date);
            let opencode_sessions = opencode_row.map_or(0, |r| r.sessions);
            let opencode_tokens = opencode_row.map_or(0, |r| r.total_tokens);

            DailyUsage {
                date: date.clone(),
                claude_sessions,
                claude_tokens,
                codex_sessions,
                codex_tokens,
                opencode_sessions,
                opencode_tokens,
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
// OpenCode SQLite
// ---------------------------------------------------------------------------

struct OpenCodeDayRow {
    day: String,
    sessions: u32,
    messages: u32,
    total_tokens: u64,
    total_cost: f64,
}

fn load_opencode_stats() -> Vec<OpenCodeDayRow> {
    let Some(path) = home_dir().map(|h| h.join(".local/share/opencode/opencode.db")) else {
        return Vec::new();
    };
    if !path.exists() {
        return Vec::new();
    }
    load_opencode_stats_from_path(&path).unwrap_or_default()
}

fn load_opencode_stats_from_path(path: &std::path::Path) -> Option<Vec<OpenCodeDayRow>> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct OpenCodeTokenCache {
        read: u64,
        write: u64,
    }

    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct OpenCodeTokens {
        total: Option<u64>,
        input: u64,
        output: u64,
        reasoning: u64,
        cache: OpenCodeTokenCache,
    }

    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct OpenCodeMessageInfo {
        role: String,
        cost: f64,
        tokens: OpenCodeTokens,
    }

    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(path, flags).ok()?;
    let mut days: std::collections::BTreeMap<String, OpenCodeDayRow> = std::collections::BTreeMap::new();

    let mut sessions_stmt = conn
        .prepare(
            "SELECT date(time_updated / 1000, 'unixepoch') AS day, count(*) AS sessions
             FROM session
             GROUP BY day
             ORDER BY day DESC
             LIMIT 30",
        )
        .ok()?;
    let session_rows = sessions_stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)))
        .ok()?;
    for row in session_rows.flatten() {
        let (day, sessions) = row;
        days.entry(day.clone()).or_insert_with(|| OpenCodeDayRow {
            day: day.clone(),
            sessions: 0,
            messages: 0,
            total_tokens: 0,
            total_cost: 0.0,
        });
        if let Some(day_row) = days.get_mut(&day) {
            day_row.sessions = sessions;
        }
    }

    let cutoff_millis = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()?
    .saturating_sub(30_i64 * 86_400_000);
    let mut messages_stmt = conn
        .prepare("SELECT time_created, data FROM message WHERE time_created >= ? ORDER BY time_created DESC")
        .ok()?;
    let message_rows = messages_stmt
        .query_map([cutoff_millis], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;
    for row in message_rows.flatten() {
        let (time_created, data) = row;
        let Ok(message) = serde_json::from_str::<OpenCodeMessageInfo>(&data) else {
            continue;
        };
        if message.role != "assistant" {
            continue;
        }
        let millis = u64::try_from(time_created.max(0)).ok()?;
        let day = unix_secs_to_date_string(millis / 1000);
        let total_tokens = message.tokens.total.unwrap_or(
            message.tokens.input
                + message.tokens.output
                + message.tokens.reasoning
                + message.tokens.cache.read
                + message.tokens.cache.write,
        );
        let day_row = days.entry(day.clone()).or_insert_with(|| OpenCodeDayRow {
            day,
            sessions: 0,
            messages: 0,
            total_tokens: 0,
            total_cost: 0.0,
        });
        day_row.messages = day_row.messages.saturating_add(1);
        day_row.total_tokens = day_row.total_tokens.saturating_add(total_tokens);
        day_row.total_cost += message.cost.max(0.0);
    }

    let mut result: Vec<_> = days.into_values().collect();
    result.sort_by(|left, right| right.day.cmp(&left.day));
    result.truncate(30);
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

    #[test]
    fn load_opencode_stats_aggregates_sessions_tokens_and_costs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&path).expect("sqlite");
        let now_millis = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix time")
                .as_millis(),
        )
        .expect("millis");
        conn.execute_batch(
            &format!(
                "\
CREATE TABLE session (
    id TEXT PRIMARY KEY,
    time_updated INTEGER NOT NULL
);
CREATE TABLE message (
    id TEXT PRIMARY KEY,
    time_created INTEGER NOT NULL,
    data TEXT NOT NULL
);
INSERT INTO session (id, time_updated) VALUES
    ('session-1', {now_millis}),
    ('session-2', {now_millis});
INSERT INTO message (id, time_created, data) VALUES
    ('message-1', {now_millis}, '{{\"role\":\"assistant\",\"cost\":0.42,\"tokens\":{{\"input\":10,\"output\":20,\"reasoning\":5,\"cache\":{{\"read\":3,\"write\":2}}}}}}'),
    ('message-2', {now_millis}, '{{\"role\":\"user\",\"cost\":99,\"tokens\":{{\"input\":1,\"output\":1,\"reasoning\":1,\"cache\":{{\"read\":1,\"write\":1}}}}}}');
",
            ),
        )
        .expect("seed");

        let rows = load_opencode_stats_from_path(&path).expect("stats");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sessions, 2);
        assert_eq!(rows[0].messages, 1);
        assert_eq!(rows[0].total_tokens, 40);
        assert!((rows[0].total_cost - 0.42).abs() < f64::EPSILON);
    }
}
