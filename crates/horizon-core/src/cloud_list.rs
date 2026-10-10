//! The cloud list: the groups of the sidebar, the status dot of each workspace and
//! its one status line. A cloud workspace shows in **Needs you**, **Cloud** or
//! **Parked** by the state of its clouds; a workspace without a cloud shows in
//! **This PC**. The UI, the CLI and MCP read the same rows.
use crate::{Panel, PanelId};

/// A group of the cloud list, in the order the sidebar shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// A cloud waits for a decision of the user, or one of its sessions ended.
    NeedsYou,
    /// A cloud that is attached, or that Horizon deploys or reconnects now.
    Cloud,
    /// A cloud whose terminals are parked, or whose worker is stopped.
    Parked,
    /// A workspace that runs on this computer.
    ThisPc,
}

impl Group {
    pub const ALL: [Self; 4] = [Self::NeedsYou, Self::Cloud, Self::Parked, Self::ThisPc];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "Needs you",
            Self::Cloud => "Cloud",
            Self::Parked => "Parked",
            Self::ThisPc => "This PC",
        }
    }

    /// The name of the group in the CLI and MCP.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs_you",
            Self::Cloud => "cloud",
            Self::Parked => "parked",
            Self::ThisPc => "this_pc",
        }
    }
}

/// The state of one cloud, from the most urgent to the least.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Condition {
    /// An operation failed.
    Failed,
    /// The cloud waits for a decision of the user, or one of its sessions ended.
    Attention,
    /// Horizon deploys, reconnects, stops or deletes the cloud now.
    Busy,
    /// Ready, with its terminals attached.
    Ready,
    /// Not deployed, or disconnected.
    Idle,
    /// Ready, with its terminals parked.
    Parked,
    /// The worker is stopped.
    Stopped,
}

impl Condition {
    #[must_use]
    pub const fn group(self) -> Group {
        match self {
            Self::Failed | Self::Attention => Group::NeedsYou,
            Self::Busy | Self::Ready | Self::Idle => Group::Cloud,
            Self::Parked | Self::Stopped => Group::Parked,
        }
    }
}

/// The status dot of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    /// An agent shows its working indicator.
    Working,
    /// Nothing runs that the user waits for.
    Idle,
    /// Horizon runs an operation on the cloud.
    Busy,
    Attention,
    Failed,
    /// Parked; `working` when an agent of the workspace still works on the worker.
    Parked {
        working: bool,
    },
    Stopped,
}

impl Dot {
    /// The name of the dot in the CLI and MCP.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Working | Self::Parked { working: true } => "working",
            Self::Idle | Self::Parked { working: false } => "idle",
            Self::Busy => "busy",
            Self::Attention => "attention",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// What Horizon knows about one cloud of a workspace.
#[derive(Clone, Debug, PartialEq)]
pub struct CloudFacts {
    /// The id of the cloud on the board.
    pub id: u32,
    /// The title of the cloud.
    pub name: String,
    pub condition: Condition,
    /// An agent session of the cloud shows its working indicator.
    pub working: bool,
    /// The status line: what the worker last showed, or what the cloud does.
    pub line: String,
    /// What the worker bills each hour now, when its provider reports a rate.
    pub hourly_rate: Option<f64>,
    /// A stop of the worker can start now: its card offers Stop.
    pub stoppable: bool,
}

impl CloudFacts {
    /// An idle cloud that a bulk stop offers: its worker runs and can stop now, no
    /// agent works on it, and it does not wait for the user.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.stoppable
            && !self.working
            && matches!(self.condition, Condition::Ready | Condition::Idle | Condition::Parked)
    }
}

/// An hourly rate as the cloud list shows it. Three decimals keep sub-cent rates apart,
/// as on a cloud card.
#[must_use]
pub fn rate_text(rate: f64) -> String {
    format!("${rate:.3}/h")
}

/// What stopping the workers of `clouds` saves each hour, said before the stop is
/// confirmed. A worker without a reported rate makes the sum a lower bound.
#[must_use]
pub fn saving_text<'a>(clouds: impl IntoIterator<Item = &'a CloudFacts>) -> String {
    let (mut sum, mut known, mut unknown) = (0.0, 0_usize, 0_usize);
    for cloud in clouds {
        match cloud.hourly_rate {
            Some(rate) => {
                sum += rate;
                known += 1;
            }
            None => unknown += 1,
        }
    }
    let without = |count: usize| {
        if count == 1 {
            "1 worker reports no rate".to_owned()
        } else {
            format!("{count} workers report no rate")
        }
    };
    match (known, unknown) {
        (0, 0) => String::new(),
        (0, _) => format!("Saving unknown: {}", without(unknown)),
        (_, 0) => format!("Saves {}", rate_text(sum)),
        _ => format!("Saves at least {}; {}", rate_text(sum), without(unknown)),
    }
}

/// One row of the cloud list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub group: Group,
    pub dot: Dot,
    pub line: String,
    /// What the workspace's workers bill each hour now; `None` when no worker reports a rate.
    pub hourly_rate: Option<f64>,
}

impl Row {
    /// The row of a workspace with the clouds `clouds`, or of a workspace on this
    /// computer when it has none. The most urgent cloud gives the group and the line.
    #[must_use]
    pub fn of(clouds: &[CloudFacts], local_working: bool, local_line: Option<&str>) -> Self {
        let Some(first) = clouds.iter().min_by_key(|cloud| cloud.condition) else {
            return Self {
                group: Group::ThisPc,
                dot: if local_working { Dot::Working } else { Dot::Idle },
                line: local_line.unwrap_or_default().to_owned(),
                hourly_rate: None,
            };
        };
        let working = clouds.iter().any(|cloud| cloud.working);
        let dot = match first.condition {
            Condition::Failed => Dot::Failed,
            Condition::Attention => Dot::Attention,
            Condition::Busy => Dot::Busy,
            Condition::Ready | Condition::Idle if working => Dot::Working,
            Condition::Ready | Condition::Idle => Dot::Idle,
            Condition::Parked => Dot::Parked { working },
            Condition::Stopped => Dot::Stopped,
        };
        let rates: Vec<f64> = clouds.iter().filter_map(|cloud| cloud.hourly_rate).collect();
        Self {
            group: first.condition.group(),
            dot,
            line: first.line.clone(),
            hourly_rate: (!rates.is_empty()).then(|| rates.iter().sum()),
        }
    }
}

/// The summary on the header of `group` with the rows `rows`, as parts from the most
/// to the least important. A narrow sidebar shows only the first parts.
#[must_use]
pub fn group_summary<'a>(group: Group, rows: impl IntoIterator<Item = &'a Row>) -> Vec<String> {
    let rate: f64 = rows.into_iter().filter_map(|row| row.hourly_rate).sum();
    let rate = (rate > 0.0).then(|| rate_text(rate));
    match group {
        Group::NeedsYou | Group::Cloud => rate.into_iter().collect(),
        Group::Parked => rate.into_iter().chain(["no local cost".to_owned()]).collect(),
        Group::ThisPc => vec!["live".to_owned()],
    }
}

/// Footer hints that agent TUIs pin below their input; they say nothing about the work.
const FOOTER_HINTS: [&str; 3] = ["for shortcuts", "shift+tab to cycle", "ctrl+t to"];
/// The longest status line, in characters.
const MAX_LINE_CHARS: usize = 120;

/// Frame and bullet characters around the text of a terminal line.
fn is_frame(character: char) -> bool {
    character.is_whitespace()
        || matches!(character,
            '\u{2500}'..='\u{259F}' // box drawing and block elements
            | '\u{25A0}'..='\u{25FF}' // geometric shapes
            | '\u{2700}'..='\u{27BF}' // dingbats, `❯` among them
            | '\u{23F5}'..='\u{23FA}' // media symbols that agents use as bullets
            | '|' | '•' | '·')
}

/// Prompt and quote characters before the text of a terminal line.
fn is_prompt(character: char) -> bool {
    matches!(character, '>' | '$' | '#' | '%' | '*' | '›' | '»')
}

/// A shell prompt with nothing typed after it, such as `user@host:~/repo$` or the
/// `host%` of zsh. A percentage at the end of a line, such as `50%`, is not one.
fn is_empty_prompt(line: &str) -> bool {
    let line = line.trim_end_matches(is_frame);
    let mut ending = line.chars().rev();
    match ending.next() {
        Some('$' | '#' | '>' | '❯') => true,
        Some('%') => !ending.next().is_some_and(|before| before.is_ascii_digit()),
        _ => false,
    }
}

/// The current window in the window list of tmux, such as `0:bash*`, also when a
/// long session name runs into it, as in `[c8dfe3af-0:bash*`.
fn is_tmux_current_window(word: &str) -> bool {
    word.split_once(':').is_some_and(|(before, window)| {
        before.ends_with(|character: char| character.is_ascii_digit()) && window.len() > 1 && window.ends_with('*')
    })
}

/// The status bar that tmux draws at the bottom of a session by default, such as
/// `[c8dfe3af-0:bash*  "host" 06:39 10-Oct-26`: it starts with the session, has
/// the current window and ends with the clock and the date.
fn is_tmux_status_bar(line: &str) -> bool {
    let mut words = line.split_whitespace().rev();
    let (Some(date), Some(clock)) = (words.next(), words.next()) else {
        return false;
    };
    let date: Vec<&str> = date.split('-').collect();
    let digits = |text: &str, count: usize| text.len() == count && text.bytes().all(|byte| byte.is_ascii_digit());
    let clock = clock
        .split_once(':')
        .is_some_and(|(hours, minutes)| digits(hours, 2) && digits(minutes, 2));
    let date = matches!(date.as_slice(), [day, month, year]
        if digits(day, 2) && month.len() == 3 && month.bytes().all(|byte| byte.is_ascii_alphabetic()) && digits(year, 2));
    line.trim_start().starts_with('[') && clock && date && line.split_whitespace().any(is_tmux_current_window)
}

/// The status line of a terminal: its last line, from `lines` oldest first, that has
/// words in it once frames, prompts and bullets are taken away. Footer hints, an
/// empty shell prompt and the status bar of tmux are not status lines.
#[must_use]
pub fn status_line<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<String> {
    wrapped_status_line(lines.into_iter().map(|line| (line, line.chars().count())), None)
}

/// Rows above the last one that a wrapped status line takes, at most.
const MAX_WRAPPED_ROWS: usize = 3;

/// As [`status_line`], for the rows of a screen `columns` wide, each with the number
/// of columns that its text occupies, blank rows included: a line that fills the
/// width of the screen continues on the next row, so the rows of a wrapped line are
/// joined. A program in tmux wraps its own lines, so the terminal cannot mark them.
#[must_use]
pub fn wrapped_status_line<'a>(
    rows: impl IntoIterator<Item = (&'a str, usize)>,
    columns: Option<usize>,
) -> Option<String> {
    let rows: Vec<(&str, usize)> = rows.into_iter().collect();
    let full = |width: usize| columns.is_some_and(|columns| columns > 1 && width + 1 >= columns);
    (0..rows.len()).rev().find_map(|last| {
        let line = rows[last].0;
        if is_empty_prompt(line) || is_tmux_status_bar(line) {
            return None;
        }
        let mut first = last;
        while first > 0 && last - first < MAX_WRAPPED_ROWS && full(rows[first - 1].1) {
            first -= 1;
        }
        // A row one short of the width lost the space at its end when it was trimmed.
        let joined = rows[first..=last]
            .iter()
            .enumerate()
            .map(|(row, (line, width))| {
                let space = row + first < last && columns.is_some_and(|columns| width + 1 == columns);
                if space { format!("{line} ") } else { (*line).to_owned() }
            })
            .collect::<String>();
        let text = joined
            .trim_start_matches(|character| is_frame(character) || is_prompt(character))
            .trim_end_matches(is_frame);
        let words = text.chars().filter(|character| character.is_alphanumeric()).count();
        let hint = FOOTER_HINTS.iter().any(|hint| text.contains(hint));
        (words >= 3 && !hint).then(|| crate::truncate_chars(text, MAX_LINE_CHARS).into_owned())
    })
}

/// The panel whose terminal speaks for a workspace: its first agent, else its
/// focused terminal, else its first terminal.
#[must_use]
pub fn primary_panel<'a>(
    panels: impl Iterator<Item = &'a Panel> + Clone,
    focused: Option<PanelId>,
) -> Option<&'a Panel> {
    let mut terminals = panels.filter(|panel| panel.terminal().is_some());
    terminals
        .clone()
        .find(|panel| panel.kind.is_agent())
        .or_else(|| terminals.clone().find(|panel| Some(panel.id) == focused))
        .or_else(|| terminals.next())
}

/// The status line of the terminal of `panel`, read from the bottom of its screen.
#[must_use]
pub fn panel_line(panel: &Panel) -> Option<String> {
    let terminal = panel.terminal()?;
    // The whole screen: output at the top of a tall screen leaves its bottom rows empty.
    let rows = terminal.screen_rows();
    wrapped_status_line(
        rows.iter().map(|(text, width)| (text.as_str(), *width)),
        Some(usize::from(terminal.cols())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud(condition: Condition, working: bool, rate: Option<f64>) -> CloudFacts {
        CloudFacts {
            id: 1,
            name: "sample".to_owned(),
            condition,
            working,
            line: format!("{condition:?}"),
            hourly_rate: rate,
            stoppable: true,
        }
    }

    #[test]
    fn a_bulk_stop_offers_only_idle_clouds_that_can_stop_now() {
        for (condition, idle) in [
            (Condition::Failed, false),
            (Condition::Attention, false),
            (Condition::Busy, false),
            (Condition::Ready, true),
            (Condition::Idle, true),
            (Condition::Parked, true),
            (Condition::Stopped, false),
        ] {
            assert_eq!(cloud(condition, false, None).idle(), idle, "{condition:?}");
        }
        assert!(!cloud(Condition::Ready, true, None).idle(), "an agent works on it");
        assert!(!cloud(Condition::Parked, true, None).idle(), "an agent works on the worker");
        let mut blocked = cloud(Condition::Ready, false, None);
        blocked.stoppable = false;
        assert!(!blocked.idle(), "its card does not offer Stop");
    }

    #[test]
    fn the_saving_of_a_bulk_stop_adds_up_the_reported_rates() {
        let rated = [
            cloud(Condition::Ready, false, Some(0.0121)),
            cloud(Condition::Parked, false, Some(0.24)),
        ];
        assert_eq!(saving_text(&rated), "Saves $0.252/h");
        let unknown = cloud(Condition::Ready, false, None);
        assert_eq!(
            saving_text([&rated[0], &unknown]),
            "Saves at least $0.012/h; 1 worker reports no rate"
        );
        assert_eq!(
            saving_text([&unknown, &unknown]),
            "Saving unknown: 2 workers report no rate"
        );
        assert_eq!(saving_text(&[]), "");
    }

    #[test]
    fn a_workspace_without_a_cloud_is_on_this_pc() {
        let row = Row::of(&[], true, Some("cargo build"));
        assert_eq!(row.group, Group::ThisPc);
        assert_eq!(row.dot, Dot::Working);
        assert_eq!(row.line, "cargo build");
        assert_eq!(row.hourly_rate, None);
        assert_eq!(Row::of(&[], false, None).dot, Dot::Idle);
    }

    #[test]
    fn each_condition_has_its_group_and_dot() {
        for (condition, group, dot) in [
            (Condition::Failed, Group::NeedsYou, Dot::Failed),
            (Condition::Attention, Group::NeedsYou, Dot::Attention),
            (Condition::Busy, Group::Cloud, Dot::Busy),
            (Condition::Ready, Group::Cloud, Dot::Idle),
            (Condition::Idle, Group::Cloud, Dot::Idle),
            (Condition::Parked, Group::Parked, Dot::Parked { working: false }),
            (Condition::Stopped, Group::Parked, Dot::Stopped),
        ] {
            let row = Row::of(&[cloud(condition, false, None)], true, None);
            assert_eq!((row.group, row.dot), (group, dot), "{condition:?}");
        }
        let working = Row::of(&[cloud(Condition::Ready, true, None)], false, None);
        assert_eq!(working.dot, Dot::Working);
        let parked = Row::of(&[cloud(Condition::Parked, true, None)], false, None);
        assert_eq!(parked.dot, Dot::Parked { working: true });
    }

    #[test]
    fn the_most_urgent_cloud_gives_the_group_and_line_and_rates_add_up() {
        let row = Row::of(
            &[
                cloud(Condition::Parked, false, Some(0.25)),
                cloud(Condition::Attention, false, Some(0.5)),
                cloud(Condition::Stopped, false, None),
            ],
            false,
            None,
        );
        assert_eq!(row.group, Group::NeedsYou);
        assert_eq!(row.line, "Attention");
        assert_eq!(row.hourly_rate, Some(0.75));
    }

    #[test]
    fn a_group_summary_adds_up_the_rates_of_its_rows() {
        let rows = [
            Row::of(&[cloud(Condition::Ready, false, Some(0.25))], false, None),
            Row::of(&[cloud(Condition::Ready, false, Some(0.0042))], false, None),
            Row::of(&[cloud(Condition::Ready, false, None)], false, None),
        ];
        assert_eq!(group_summary(Group::Cloud, &rows), ["$0.254/h"]);
        assert_eq!(group_summary(Group::Parked, &rows), ["$0.254/h", "no local cost"]);
        assert!(group_summary(Group::NeedsYou, &rows[2..]).is_empty());
        assert_eq!(group_summary(Group::Parked, &rows[2..]), ["no local cost"]);
        assert_eq!(group_summary(Group::ThisPc, &[]), ["live"]);
    }

    #[test]
    fn the_status_line_skips_frames_prompts_and_footer_hints() {
        let screen = [
            "● Edit 2 files (+41 -7)",
            "╭──────────────────────╮",
            "│ >                    │",
            "╰──────────────────────╯",
            "  ? for shortcuts",
        ];
        assert_eq!(
            status_line(screen.iter().copied()).as_deref(),
            Some("Edit 2 files (+41 -7)")
        );
        assert_eq!(
            status_line(["$ cargo test -p core", "$ "]).as_deref(),
            Some("cargo test -p core")
        );
        assert_eq!(status_line(["", "──", "> a"]), None);
        assert_eq!(
            status_line(["Finished dev profile", "user@host:~/repo$"]).as_deref(),
            Some("Finished dev profile")
        );
        assert_eq!(
            status_line(["synthetic output 12", "[c8dfe3af-0:bash*   \"example\" 06:39 10-Oct-26"]).as_deref(),
            Some("synthetic output 12"),
            "the tmux status bar of a cloud session is not a status line"
        );
        for log in [
            "[INFO] build done at 06:39 on 10-Oct-26",
            "[INFO] build done 06:39 10-Oct-26",
        ] {
            assert_eq!(
                status_line([log]).as_deref(),
                Some(log),
                "a log line with a clock is a status line"
            );
        }
        assert_eq!(status_line(["% cargo test", "host%"]).as_deref(), Some("cargo test"));
        assert_eq!(
            status_line(["Downloading layers 50%"]).as_deref(),
            Some("Downloading layers 50%")
        );
    }

    fn wrapped(rows: &[&str], columns: usize) -> Option<String> {
        wrapped_status_line(rows.iter().map(|row| (*row, row.chars().count())), Some(columns))
    }

    #[test]
    fn a_line_that_fills_the_screen_continues_on_the_next_row() {
        let screen = [
            "synthetic output 11",
            "Session process exited with status 0. Reconnect",
            "preserves this result.",
        ];
        // The first row ended with a space at the edge of a screen 48 columns wide.
        assert_eq!(
            wrapped(&screen, 48).as_deref(),
            Some("Session process exited with status 0. Reconnect preserves this result.")
        );
        // A row that fills the whole width broke inside a word.
        assert_eq!(
            wrapped(&["Downloading the wor", "ker image"], 19).as_deref(),
            Some("Downloading the worker image")
        );
        // Short rows are separate lines.
        assert_eq!(
            wrapped(&["synthetic output 11", "synthetic output 12"], 80).as_deref(),
            Some("synthetic output 12")
        );
        // A blank row ends a line, also after a row that fills the width.
        assert_eq!(
            wrapped(&["Downloading the wor", "", "synthetic output 12"], 19).as_deref(),
            Some("synthetic output 12")
        );
        // Wide glyphs fill a row in fewer characters than columns.
        let wide = "\u{4e2d}".repeat(9);
        let rows = [(wide.as_str(), 18), ("is the worker log", 17)];
        assert_eq!(
            wrapped_status_line(rows, Some(19)).as_deref(),
            Some(format!("{wide} is the worker log").as_str())
        );
    }

    #[test]
    fn a_long_status_line_is_shortened() {
        let long = "word ".repeat(60);
        let line = status_line([long.as_str()]).unwrap();
        assert_eq!(line.chars().count(), MAX_LINE_CHARS);
    }
}
